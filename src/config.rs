//! Settings from ~/.config/wm-dash.json. Every key is optional.

use anyhow::{Context, Result, ensure};
use std::sync::OnceLock;

#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub layout_ratio: Layout,
}

/// How the width is shared between the agent list and the preview, as a ratio: 30/70, 3/7 and 1/2 all work.
#[derive(serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub list: u16,
    pub preview: u16,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { list: 30, preview: 70 }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Read the file once at startup. A missing file means defaults; a broken one is an error.
pub fn load() -> Result<()> {
    let path = std::path::Path::new(&std::env::var("HOME")?).join(".config/wm-dash.json");
    let config = match std::fs::read_to_string(&path) {
        Ok(s) => parse(&s).with_context(|| format!("in {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let _ = CONFIG.set(config);
    Ok(())
}

fn parse(s: &str) -> Result<Config> {
    let config: Config = serde_json::from_str(s)?;
    let Layout { list, preview } = config.layout_ratio;
    ensure!(list > 0 && preview > 0, "layout_ratio.list and layout_ratio.preview must both be above 0");
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse("{}").unwrap().layout_ratio.list, 30);
        let c = parse(r#"{"layout_ratio": {"list": 1, "preview": 2}}"#).unwrap();
        assert_eq!((c.layout_ratio.list, c.layout_ratio.preview), (1, 2));
        assert!(parse(r#"{"layout_ratio": {"list": 0}}"#).is_err());
        assert!(parse(r#"{"layout_ratio": {"lst": 40}}"#).is_err());
    }
}
