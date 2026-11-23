use asrs_js::string::JsString;

/// The address the window's `start` message carries: each dot-separated part of the preferred
/// address without its leading zeros (upstream issue 167).
pub fn normalize_address(address: &JsString) -> JsString {
    let dot = JsString::from(".");
    let mut out = JsString::new();
    for (i, part) in address.split(Some(&dot), None).iter().enumerate() {
        if i > 0 {
            out.push_str(&dot);
        }
        let zeros = part
            .units()
            .iter()
            .take_while(|unit| **unit == u16::from(b'0'))
            .count();
        let rest = &part.units()[zeros..];
        if rest.is_empty() {
            out.push_str(&JsString::from("0"));
        } else {
            out.push_str(&JsString::from_units(rest.to_vec()));
        }
    }
    out
}

/// The HTML `show-error` puts in the alert. `address` is the normalized one.
pub fn friendly_error(message: &str, address: &JsString, remote_path: &JsString) -> JsString {
    let around = |before: &str, value: &JsString, after: &str| {
        let mut out = JsString::from(before);
        out.push_str(value);
        out.push_str(&JsString::from(after));
        out
    };
    match message {
        "No such file" => around("Failed to open log folder at <u>", remote_path, "</u>"),
        "Timed out while waiting for handshake" | "Fetch failed" => {
            around("Robot not found at <u>", address, "</u> (check connection)")
        }
        _ if message.contains("ENOTFOUND") => around("Unknown address <u>", address, "</u>"),
        "All configured authentication methods failed" => {
            around("Failed to authenticate to robot at <u>", address, "</u>")
        }
        "Not connected" => JsString::from("Lost connection to robot"),
        _ => JsString::from(format!("Unknown error: {message}")),
    }
}
