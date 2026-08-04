use anyhow::Result;

pub fn format_status(json: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let reqs = v["requests"].as_u64().unwrap_or(0);
    let masked = v["masked"].as_u64().unwrap_or(0);
    let redacted = v["redacted"].as_u64().unwrap_or(0);
    let alerts = v["alerts"].as_u64().unwrap_or(0);
    Ok(format!(
        "proxy: up\nrequests: {reqs}\nmasked: {masked}\nredacted: {redacted}\nalerts: {alerts}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_status_renders_json() {
        let out = format_status(r#"{"requests":3,"masked":2,"redacted":1,"alerts":1}"#).unwrap();
        assert!(out.contains("requests: 3"));
        assert!(out.contains("redacted: 1"));
    }
}
