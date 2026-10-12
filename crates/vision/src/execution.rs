//! Per-analysis execution preference; no process-global environment changes.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    #[default]
    Auto,
    Cpu,
    Gpu,
}
impl Device {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_device_values_are_rejected() {
        assert!(serde_json::from_str::<Device>("\"mps\"").is_err());
        assert_eq!(serde_json::from_str::<Device>("\"gpu\"").unwrap(), Device::Gpu);
    }
}
