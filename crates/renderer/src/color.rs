use alacritty_config_derive::SerdeReplace;
use alacritty_terminal::vte::ansi::Rgb as VteRgb;
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Display, Formatter};
use std::ops::{Add, Deref, Mul};
use std::str::FromStr;

#[derive(SerdeReplace, Debug, Eq, PartialEq, Copy, Clone, Default)]
pub struct Rgb(pub VteRgb);

impl Rgb {
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(VteRgb { r, g, b })
    }

    #[inline]
    pub fn as_tuple(self) -> (u8, u8, u8) {
        (self.0.r, self.0.g, self.0.b)
    }
}

impl From<VteRgb> for Rgb {
    fn from(value: VteRgb) -> Self {
        Self(value)
    }
}

impl Deref for Rgb {
    type Target = VteRgb;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Mul<f32> for Rgb {
    type Output = Rgb;

    fn mul(self, rhs: f32) -> Self::Output {
        Rgb(self.0 * rhs)
    }
}

impl Add<Rgb> for Rgb {
    type Output = Rgb;

    fn add(self, rhs: Rgb) -> Self::Output {
        Rgb(self.0 + rhs.0)
    }
}

/// Deserialize Rgb color from a hex string.
impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RgbVisitor;

        // Used for deserializing reftests.
        #[derive(Deserialize)]
        struct RgbDerivedDeser {
            r: u8,
            g: u8,
            b: u8,
        }

        impl Visitor<'_> for RgbVisitor {
            type Value = Rgb;

            fn expecting(&self, f: &mut Formatter<'_>) -> fmt::Result {
                f.write_str("hex color like #ff00ff")
            }

            fn visit_str<E>(self, value: &str) -> Result<Rgb, E>
            where
                E: serde::de::Error,
            {
                Rgb::from_str(value).map_err(|_| {
                    E::custom(format!(
                        "failed to parse rgb color {value}; expected hex color like #ff00ff"
                    ))
                })
            }
        }

        // Return an error if the syntax is incorrect.
        let value = toml::Value::deserialize(deserializer)?;

        // Attempt to deserialize from struct form.
        if let Ok(RgbDerivedDeser { r, g, b }) = RgbDerivedDeser::deserialize(value.clone()) {
            return Ok(Rgb::new(r, g, b));
        }

        // Deserialize from hex notation (either 0xff00ff or #ff00ff).
        value.deserialize_str(RgbVisitor).map_err(D::Error::custom)
    }
}

/// Serialize Rgb color to a hex string.
impl Serialize for Rgb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for Rgb {
    type Err = ();

    fn from_str(s: &str) -> Result<Rgb, ()> {
        let chars = if s.starts_with("0x") && s.len() == 8 {
            &s[2..]
        } else if s.starts_with('#') && s.len() == 7 {
            &s[1..]
        } else {
            return Err(());
        };

        match u32::from_str_radix(chars, 16) {
            Ok(mut color) => {
                let b = (color & 0xff) as u8;
                color >>= 8;
                let g = (color & 0xff) as u8;
                color >>= 8;
                let r = color as u8;
                Ok(Rgb::new(r, g, b))
            },
            Err(_) => Err(()),
        }
    }
}
