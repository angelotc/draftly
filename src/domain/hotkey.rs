use std::fmt;
use std::sync::RwLock;

static CURRENT: RwLock<Hotkey> = RwLock::new(Hotkey::DEFAULT);

/// The live show/hide combo; the listener and labels read this.
pub fn current() -> Hotkey {
    *CURRENT.read().unwrap()
}

pub fn set_current(hk: Hotkey) {
    *CURRENT.write().unwrap() = hk;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Windows virtual-key code: A-Z, 0-9 or F1-F12.
    pub vk: u32,
}

impl Hotkey {
    pub const fn new(ctrl: bool, shift: bool, alt: bool, vk: u32) -> Self {
        Self { ctrl, shift, alt, vk }
    }

    pub const DEFAULT: Hotkey = Hotkey::new(true, true, false, b'L' as u32);

    pub fn is_f_key(&self) -> bool {
        (0x70..=0x7B).contains(&self.vk)
    }

    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.shift || self.alt
    }

    /// A combo is usable if its key is supported and it has a modifier (F-keys may go bare).
    pub fn is_valid(&self) -> bool {
        let key_ok = self.is_f_key() || (0x30..=0x39).contains(&self.vk) || (0x41..=0x5A).contains(&self.vk);
        key_ok && (self.has_modifier() || self.is_f_key())
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut hk = Hotkey::new(false, false, false, 0);
        let mut key: Option<u32> = None;
        for part in text.split('+') {
            let part = part.trim().to_ascii_lowercase();
            match part.as_str() {
                "ctrl" | "control" => hk.ctrl = true,
                "shift" => hk.shift = true,
                "alt" => hk.alt = true,
                "" => return Err(format!("Can't read \"{}\".", text.trim())),
                _ => {
                    if key.is_some() {
                        return Err("Use one key plus modifiers.".to_string());
                    }
                    key = Some(parse_key(&part).ok_or_else(|| format!("Unknown key \"{part}\"."))?);
                }
            }
        }
        hk.vk = key.ok_or_else(|| "Add a key.".to_string())?;
        if !hk.is_valid() {
            return Err("Add Ctrl, Shift or Alt (only F1-F12 can be used alone).".to_string());
        }
        Ok(hk)
    }
}

fn parse_key(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() == 1 && b[0].is_ascii_lowercase() {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if b.len() == 1 && b[0].is_ascii_digit() {
        return Some(b[0] as u32);
    }
    let n: u32 = s.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&n).then(|| 0x70 + n - 1)
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.is_f_key() {
            write!(f, "F{}", self.vk - 0x70 + 1)
        } else {
            write!(f, "{}", char::from_u32(self.vk).unwrap_or('?'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_round_trip() {
        for s in ["Ctrl+Shift+L", "Ctrl+X", "Alt+5", "F9", "Ctrl+Alt+F12"] {
            assert_eq!(Hotkey::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn parse_tolerates_case_and_spacing() {
        let hk = Hotkey::parse("  CTRL + shift+ l ").unwrap();
        assert_eq!(hk, Hotkey::DEFAULT);
        assert_eq!(Hotkey::parse("shift+ctrl+L").unwrap(), hk);
    }

    #[test]
    fn rejects_bare_letters_and_bad_input() {
        assert!(Hotkey::parse("l").is_err());
        assert!(Hotkey::parse("5").is_err());
        assert!(Hotkey::parse("ctrl").is_err());
        assert!(Hotkey::parse("ctrl+").is_err());
        assert!(Hotkey::parse("ctrl+a+b").is_err());
        assert!(Hotkey::parse("ctrl+f13").is_err());
        assert!(Hotkey::parse("ctrl+;").is_err());
        assert!(Hotkey::parse("f5").is_ok());
    }
}
