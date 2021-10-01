#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: String| {
    let serialized = nixlike::serialize(data.clone()).unwrap();
    let deserialized: String = nixlike::parse_str(&serialized).unwrap();
    
    assert_eq!(data, deserialized);
});
