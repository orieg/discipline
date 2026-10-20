//! Fuzz target for `forge` remote/SSH/URL/path parsers and `instruction_smuggling`
//! Unicode, homoglyph, and encoded-phrase detectors.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = discipline::forge::parse_remote(&text);
    let _ = discipline::forge::clean_url(&text);
    let _ = discipline::forge::encode_segment(&text);
    let _ = discipline::forge::check_api_path(&text);

    let mid = text.len() / 2;
    let split = text.floor_char_boundary(mid);
    let (cfg, alias) = text.split_at(split);
    let _ = discipline::forge::ssh_hostname(cfg, alias.trim());

    let _ = discipline::guards::instruction_smuggling::invisible_classes(&text, true);
    let _ = discipline::guards::instruction_smuggling::invisible_classes(&text, false);
    let _ = discipline::guards::instruction_smuggling::phrase_classes(&text);
    let _ = discipline::guards::instruction_smuggling::fold(&text);
    let _ = discipline::guards::instruction_smuggling::has_mixed_script_word(&text);
    let _ = discipline::guards::instruction_smuggling::has_encoded_blob(&text);
    let _ = discipline::guards::instruction_smuggling::text_classes(&text);
});
