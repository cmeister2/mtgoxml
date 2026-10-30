//! Named client color flags and conversion from server property values.

use std::ops::BitOr;

/// The client's `MagicColors` flags, distinct from its server color bit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ColorFlags(u8);

impl ColorFlags {
    /// No selected color flags.
    const INVALID: Self = Self(0);
    /// White mana.
    pub(super) const WHITE: Self = Self(1);
    /// Blue mana.
    pub(super) const BLUE: Self = Self(2);
    /// Black mana.
    pub(super) const BLACK: Self = Self(4);
    /// Red mana.
    pub(super) const RED: Self = Self(8);
    /// Green mana.
    pub(super) const GREEN: Self = Self(16);
    /// The explicit colorless flag, which can also appear alongside colors.
    pub(super) const COLORLESS: Self = Self(32);
    /// All five mana colors, without the colorless flag.
    const FIVE_COLOR: Self =
        Self(Self::WHITE.0 | Self::BLUE.0 | Self::BLACK.0 | Self::RED.0 | Self::GREEN.0);

    /// Parse named property colors or a signed server bitmask.
    pub(super) fn from_spec(spec: &str) -> Option<Self> {
        if spec.trim().is_empty() {
            return Some(Self::COLORLESS);
        }
        if let Ok(server) = spec.trim().parse::<i32>() {
            return Some(Self::from_server(server));
        }
        spec.split('|').try_fold(Self::INVALID, |color, part| {
            // Named properties use client flags, including explicit colorless.
            Some(
                color
                    | match part.trim() {
                        "COLOR_WHITE" => Self::WHITE,
                        "COLOR_BLUE" => Self::BLUE,
                        "COLOR_BLACK" => Self::BLACK,
                        "COLOR_RED" => Self::RED,
                        "COLOR_GREEN" => Self::GREEN,
                        "COLOR_COLORLESS" | "" | "0" => Self::COLORLESS,
                        _ => return None,
                    },
            )
        })
    }

    /// Translate the server's flags using `GetColorsFromServerColors` semantics.
    fn from_server(server: i32) -> Self {
        if ServerColor::Any.is_set(server) {
            return Self::FIVE_COLOR;
        }
        let mut color = Self::INVALID;
        for (server_color, client_color) in [
            (ServerColor::White, Self::WHITE),
            (ServerColor::Blue, Self::BLUE),
            (ServerColor::Black, Self::BLACK),
            (ServerColor::Red, Self::RED),
            (ServerColor::Green, Self::GREEN),
        ] {
            // Ignore server bits that do not identify a Magic color.
            if server_color.is_set(server) {
                color = color | client_color;
            }
        }
        if color == Self::INVALID {
            Self::COLORLESS
        } else {
            color
        }
    }
}

impl BitOr for ColorFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// The client's `MagicServerColors` bits, as encoded in numeric properties.
#[derive(Clone, Copy)]
#[repr(i32)]
enum ServerColor {
    /// Black mana in server properties.
    Black = 1,
    /// Blue mana in server properties.
    Blue = 2,
    /// Green mana in server properties.
    Green = 4,
    /// Red mana in server properties.
    Red = 8,
    /// White mana in server properties.
    White = 16,
    /// Any color, overriding individual bits with all five colors.
    Any = 32,
}

impl ServerColor {
    /// Whether this server flag is present in a signed property value.
    fn is_set(self, server: i32) -> bool {
        server & self as i32 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::ColorFlags;

    #[test]
    fn server_and_named_colors_use_their_own_bit_orders() {
        assert_eq!(ColorFlags::from_spec("1"), Some(ColorFlags::BLACK));
        assert_eq!(ColorFlags::from_spec("16"), Some(ColorFlags::WHITE));
        assert_eq!(
            ColorFlags::from_spec("COLOR_WHITE|COLOR_BLUE"),
            ColorFlags::from_spec("18"),
        );
        assert_eq!(ColorFlags::from_spec("32"), Some(ColorFlags::FIVE_COLOR));
        assert_eq!(ColorFlags::from_spec("-1"), Some(ColorFlags::FIVE_COLOR));
    }

    #[test]
    fn colorless_is_a_distinct_flag_and_unknown_names_are_rejected() {
        assert_eq!(ColorFlags::from_spec(""), Some(ColorFlags::COLORLESS));
        assert_eq!(ColorFlags::from_spec("0"), Some(ColorFlags::COLORLESS));
        assert_eq!(ColorFlags::from_spec("64"), Some(ColorFlags::COLORLESS));
        assert_eq!(
            ColorFlags::from_spec("COLOR_WHITE|COLOR_COLORLESS"),
            Some(ColorFlags::WHITE | ColorFlags::COLORLESS),
        );
        assert_eq!(ColorFlags::from_spec("COLOR_UNKNOWN"), None);
    }
}
