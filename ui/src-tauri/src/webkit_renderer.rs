use std::ffi::OsStr;

const SHARED_MEMORY: &[(&str, &str)] = &[("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1")];
const LEGACY: &[(&str, &str)] = &[
    ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
];

fn defaults(major: u32, minor: u32) -> &'static [(&'static str, &'static str)] {
    if (major, minor) >= (2, 44) {
        SHARED_MEMORY
    } else {
        LEGACY
    }
}

fn version() -> (u32, u32, u32) {
    // These C queries only return the loaded library's version; no GTK init.
    unsafe {
        (
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        )
    }
}

pub(crate) fn configure() {
    let (major, minor, micro) = version();
    for (key, value) in defaults(major, minor) {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    }
    // Since WebKitGTK 2.44 FORCE_SHM selects shared-memory buffers before
    // hardware/DMABUF transport is added. Disabling the renderer instead uses
    // a legacy backing store that can discard alpha even when snapshots pass.
    tracing::info!(
        "WebKitGTK {major}.{minor}.{micro}: shared-memory renderer={}, legacy renderer override={}",
        enabled(std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM").as_deref()),
        transparency_unavailable_reason().is_some(),
    );
}

fn enabled(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| value != "0")
}

fn unavailable_reason(
    major: u32,
    minor: u32,
    read: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<String> {
    if (major, minor) < (2, 44) {
        return Some("Background transparency needs WebKitGTK 2.44 or newer; using the safe opaque renderer.".into());
    }
    for (key, _) in LEGACY {
        if enabled(read(key).as_deref()) {
            return Some(format!(
                "Background transparency is unavailable because {key} disables the alpha-capable renderer. Grafium preserves this environment override and stays opaque; remove the override and quit/reopen to use the shared-memory renderer."
            ));
        }
    }
    None
}

pub(crate) fn transparency_unavailable_reason() -> Option<String> {
    let (major, minor, _) = version();
    unavailable_reason(major, minor, |name| std::env::var_os(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_memory_replaces_disabled_compositing_only_on_supported_engines() {
        assert_eq!(defaults(2, 42), LEGACY);
        assert_eq!(defaults(2, 44), SHARED_MEMORY);
        assert_eq!(defaults(2, 52), SHARED_MEMORY);
        assert_eq!(defaults(3, 0), SHARED_MEMORY);
    }

    #[test]
    fn explicit_legacy_overrides_are_diagnosed_without_changing_them() {
        for (key, _) in LEGACY {
            for value in ["1", "", "true"] {
                let reason = unavailable_reason(2, 52, |name| (name == *key).then(|| value.into()));
                assert!(reason.unwrap().contains(key));
            }
        }
        assert!(unavailable_reason(2, 52, |_| Some("0".into())).is_none());
        assert!(unavailable_reason(2, 52, |_| None).is_none());
        assert!(unavailable_reason(2, 42, |_| None)
            .unwrap()
            .contains("2.44"));
    }
}
