#![no_main]

use extractor_url_routing::{RoutingPolicy, classify, normalize};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = std::str::from_utf8(data) {
        if let Ok(normalized) = normalize(value, &RoutingPolicy::default()) {
            let _ = classify(&normalized);
        }
    }
});
