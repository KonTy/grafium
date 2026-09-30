//! Length-prefixed JSON transport. Stdout is reserved for these frames; hosts
//! must send diagnostics to stderr and impose limits in both directions.

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::error::{Result, RuntimeError};

pub const DEFAULT_REQUEST_LIMIT: u64 = 4 * 1024 * 1024;
pub const DEFAULT_RESPONSE_LIMIT: u64 = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "O: DeserializeOwned, P: DeserializeOwned"))]
#[serde(deny_unknown_fields)]
pub struct Response<O, P> {
    pub output: Option<O>,
    pub error: Option<String>,
    #[serde(default)]
    pub progress: Option<P>,
}

pub enum Event<O, P> {
    Output(O),
    Progress(P),
}

impl<O, P> Response<O, P> {
    pub fn is_progress(&self) -> bool {
        self.progress.is_some() && self.output.is_none() && self.error.is_none()
    }

    /// Validate before returning a worker to the reuse pool, including errors
    /// that arrived in a syntactically valid transport frame.
    pub fn into_event(self) -> Result<Event<O, P>> {
        match (self.output, self.error, self.progress) {
            (Some(output), None, None) => Ok(Event::Output(output)),
            (None, None, Some(progress)) => Ok(Event::Progress(progress)),
            (None, Some(error), None) => Err(RuntimeError::Other(error)),
            _ => Err(RuntimeError::Other(
                "native worker returned an invalid response".into(),
            )),
        }
    }

    pub fn into_output(self) -> Result<O> {
        match self.into_event()? {
            Event::Output(output) => Ok(output),
            Event::Progress(_) => Err(RuntimeError::Other(
                "expected a terminal native worker response".into(),
            )),
        }
    }
}

pub fn write_bytes(writer: &mut impl Write, payload: &[u8], limit: u64) -> io::Result<()> {
    if payload.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "worker frame exceeds size limit",
        ));
    }
    writer.write_all(&(payload.len() as u64).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

pub fn write_frame<T: Serialize>(
    writer: &mut impl Write,
    message: &T,
    limit: u64,
) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_bytes(writer, &payload, limit)
}

pub fn read_bytes(reader: &mut impl Read, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 8];
    // EOF is graceful only between frames, never in a partial header/body.
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let len = u64::from_le_bytes(header);
    if len > limit || len > usize::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "worker frame exceeds size limit",
        ));
    }
    let mut payload = vec![0; len as usize];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

pub fn read_frame<T: DeserializeOwned>(
    reader: &mut impl Read,
    limit: u64,
) -> io::Result<Option<T>> {
    read_bytes(reader, limit)?
        .map(|payload| serde_json::from_slice(&payload).map_err(io::Error::other))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_truncation_are_checked_before_decoding() {
        assert!(read_bytes(&mut &[][..], 100).unwrap().is_none());
        assert_eq!(
            read_bytes(&mut &[1, 0][..], 100).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(read_bytes(&mut &101_u64.to_le_bytes()[..], 100).is_err());
        assert!(read_bytes(&mut &3_u64.to_le_bytes()[..], 100).is_err());
        let mut frame = Vec::new();
        write_frame(&mut frame, &"hello", 100).unwrap();
        assert_eq!(
            read_frame::<String>(&mut frame.as_slice(), 100).unwrap(),
            Some("hello".into())
        );
        assert!(write_frame(&mut Vec::new(), &"hello", 1).is_err());
    }

    #[test]
    fn ambiguous_and_error_responses_are_not_successes() {
        for payload in [
            r#"{}"#,
            r#"{"output":1,"error":"bad"}"#,
            r#"{"output":1,"progress":2}"#,
            r#"{"error":"bad"}"#,
        ] {
            assert!(serde_json::from_str::<Response<u32, u32>>(payload)
                .unwrap()
                .into_output()
                .is_err());
        }
    }
}
