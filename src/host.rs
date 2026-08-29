//! FFI layer: Peckboard core host functions this plugin calls. Same envelope
//! contract as the other Rust plugins: JSON-string-in / JSON-string-out, an
//! `{"error": ...}` reply surfaced as `Err(String)`.

pub enum HostFn {
    StorePut,
    StoreGet,
    StoreList,
    StoreDelete,
    CallerScope,
    CreateCard,
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::HostFn;
    use extism_pdk::*;

    #[host_fn]
    extern "ExtismHost" {
        fn peckboard_store_put(input: String) -> String;
        fn peckboard_store_get(input: String) -> String;
        fn peckboard_store_list(input: String) -> String;
        fn peckboard_store_delete(input: String) -> String;
        fn peckboard_caller_scope(input: String) -> String;
        fn peckboard_create_card(input: String) -> String;
    }

    pub fn call_host(
        which: HostFn,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let s = input.to_string();
        let out = unsafe {
            match which {
                HostFn::StorePut => peckboard_store_put(s),
                HostFn::StoreGet => peckboard_store_get(s),
                HostFn::StoreList => peckboard_store_list(s),
                HostFn::StoreDelete => peckboard_store_delete(s),
                HostFn::CallerScope => peckboard_caller_scope(s),
                HostFn::CreateCard => peckboard_create_card(s),
            }
        }
        .map_err(|e| e.to_string())?;
        let v: serde_json::Value =
            serde_json::from_str(&out).map_err(|e| format!("host returned invalid json: {e}"))?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return Err(err.to_string());
        }
        Ok(v)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::HostFn;

    pub fn call_host(
        _which: HostFn,
        _input: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        unimplemented!("host calls are only available on wasm32")
    }
}

pub use imp::call_host;
