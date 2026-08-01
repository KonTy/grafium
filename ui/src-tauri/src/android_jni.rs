//! JNI bridge so the Android `AssistantReceiver` (Kotlin) can call the same
//! Rust NLU (`grafium_core::assistant::handle_command`) as the desktop Tauri
//! command. This is the industry-standard "one Rust core, thin platform
//! shims" pattern used by e.g. Signal and 1Password.
//!
//! On Android, `libgrafium_lib.so` is already loaded by the Tauri WryActivity
//! at process start, so the Kotlin receiver just needs to declare an
//! `external fun` matching the signature below.
//!
//! Contract:
//! * Input:  `transcript: String`, `graph_path: String` (absolute path to the
//!           graph root).
//! * Output: JSON-encoded [`grafium_core::AssistantResponse`], i.e.
//!           `{"speech": "...", "followup": false}`.
//! * Errors: on any failure a fallback JSON `{"speech": "<msg>",
//!           "followup": false}` is returned so the receiver always has a
//!           speakable string.

#![cfg(target_os = "android")]

use grafium_core::Graph;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use std::path::Path;

/// Open the graph at `graph_path` using Grafium's standard `.grafium/` metadata
/// directory, hand the transcript to the shared NLU, and return the response
/// as a JSON string. Never panics: any error is wrapped in a friendly
/// [`grafium_core::AssistantResponse`] and returned as JSON.
#[no_mangle]
pub extern "system" fn Java_com_grafium_app_AssistantReceiver_nativeHandleCommand<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    j_transcript: JString<'local>,
    j_graph_path: JString<'local>,
) -> jstring {
    let transcript: String = match env.get_string(&j_transcript) {
        Ok(s) => s.into(),
        Err(_) => return json_err(&mut env, "Bad transcript argument"),
    };
    let graph_path: String = match env.get_string(&j_graph_path) {
        Ok(s) => s.into(),
        Err(_) => return json_err(&mut env, "Bad graph path"),
    };

    let response = match open_and_dispatch(&graph_path, &transcript) {
        Ok(r) => r,
        Err(e) => grafium_core::AssistantResponse {
            speech: format!("Sorry, I couldn't complete that: {}", e),
            followup: false,
        },
    };

    let json = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"speech":"Sorry, something went wrong.","followup":false}"#.to_string()
    });
    string_to_jstring(&mut env, &json)
}

fn open_and_dispatch(
    graph_path: &str,
    transcript: &str,
) -> Result<grafium_core::AssistantResponse, String> {
    let root = Path::new(graph_path);
    if !root.exists() {
        return Err(format!("Graph path does not exist: {}", graph_path));
    }
    // Always use .grafium/index.db so we match the desktop/Tauri path exactly.
    let db_path = root.join(".grafium").join("index.db");
    let graph = Graph::open_with_db_path_and_metadata_dir(root, &db_path, ".grafium")
        .map_err(|e| e.to_string())?;
    grafium_core::assistant::handle_command(&graph, transcript).map_err(|e| e.to_string())
}

fn json_err(env: &mut JNIEnv, msg: &str) -> jstring {
    let s = serde_json::to_string(&grafium_core::AssistantResponse {
        speech: msg.to_string(),
        followup: false,
    })
    .unwrap_or_else(|_| r#"{"speech":"error","followup":false}"#.to_string());
    string_to_jstring(env, &s)
}

fn string_to_jstring(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|js| js.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
