//! Accent theme color for the UI.

use serde::{Deserialize, Serialize};

/// App accent theme color: a hue family applied to the dashboard highlights and
/// the chart series palette. Persisted by key in the settings table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeColor {
    #[default]
    Ocean,
    Jade,
    Amber,
    Rose,
}

impl ThemeColor {
    pub const ALL: [ThemeColor; 4] = [Self::Ocean, Self::Jade, Self::Amber, Self::Rose];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ocean => "海蓝",
            Self::Jade => "青玉",
            Self::Amber => "琥珀",
            Self::Rose => "玫瑰",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Ocean => "ocean",
            Self::Jade => "jade",
            Self::Amber => "amber",
            Self::Rose => "rose",
        }
    }

    /// Map a persisted key back to a color; unknown values fall back to the
    /// default (ocean).
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|c| c.key() == key)
            .unwrap_or_default()
    }

    /// Base accent color as `0xRRGGBB`.
    pub fn accent(self) -> u32 {
        match self {
            Self::Ocean => 0x3b82f6,
            Self::Jade => 0x10b981,
            Self::Amber => 0xf59e0b,
            Self::Rose => 0xf43f5e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_key_round_trips_and_falls_back_to_ocean() {
        for color in ThemeColor::ALL {
            assert_eq!(ThemeColor::from_key(color.key()), color);
        }
        assert_eq!(ThemeColor::from_key("unknown"), ThemeColor::Ocean);
    }

    #[test]
    fn accents_are_distinct_hex() {
        let accents: Vec<u32> = ThemeColor::ALL.iter().map(|c| c.accent()).collect();
        let mut unique = accents.clone();
        unique.dedup();
        assert_eq!(unique.len(), accents.len());
    }
}
