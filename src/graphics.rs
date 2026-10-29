use std::sync::OnceLock;

#[derive(Debug, Default, Clone, Copy)]
struct Capabilities {
    kitty_terminal: bool,
    graphics: bool,
}

static CAPABILITIES: OnceLock<Capabilities> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsMode {
    Auto,
    Kitty,
    Unavailable,
}

impl GraphicsMode {
    pub fn detect(self) -> Self {
        if self != Self::Auto {
            return self;
        }
        let capabilities = CAPABILITIES.get().copied().unwrap_or_default();
        if capabilities.kitty_terminal && capabilities.graphics {
            Self::Kitty
        } else {
            Self::Unavailable
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Kitty => "KITTY",
            Self::Unavailable => "UNSUPPORTED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "kitty" | "pixel" => Some(Self::Kitty),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let capabilities = CAPABILITIES.get().copied().unwrap_or_default();
        match self {
            Self::Auto if capabilities.kitty_terminal && capabilities.graphics => Self::Kitty,
            _ => Self::Auto,
        }
    }

    pub fn verified(self) -> Result<Self, String> {
        let capabilities = CAPABILITIES.get().copied().unwrap_or_default();
        let supported = match self {
            Self::Auto | Self::Kitty => capabilities.kitty_terminal && capabilities.graphics,
            Self::Unavailable => false,
        };
        if supported {
            Ok(self)
        } else {
            Err("Kitty is required: this terminal did not confirm both Kitty identity and pixel graphics. Open ForgeTUI in the Kitty terminal emulator; any shell is supported.".into())
        }
    }
}

/// Query actual graphics support instead of trusting TERM or inherited environment variables.
#[cfg(unix)]
pub fn verify_terminal(requested: GraphicsMode) -> Result<GraphicsMode, String> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::{
        fs::OpenOptions,
        io::{Read, Write},
        os::unix::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/tty")
        .map_err(|error| {
            format!("The Kitty terminal emulator is required: {error}. Open Kitty and run ForgeTUI; any shell is supported.")
        })?;
    enable_raw_mode().map_err(|error| format!("Cannot query terminal: {error}"))?;
    let result = (|| -> std::io::Result<Capabilities> {
        tty.write_all(b"\x1b[>0q\x1b_Gi=48391,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c")?;
        tty.flush()?;
        let deadline = Instant::now() + Duration::from_millis(1200);
        let mut response = Vec::new();
        let mut buffer = [0; 1024];
        while Instant::now() < deadline && response.len() < 8192 {
            match tty.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    response.extend_from_slice(&buffer[..count]);
                    if response.windows(3).any(|window| window == b"\x1b[?")
                        && response.ends_with(b"c")
                    {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(parse_capabilities(&response))
    })();
    let restored = disable_raw_mode();
    let capabilities =
        result.map_err(|error| format!("Terminal graphics query failed: {error}"))?;
    restored.map_err(|error| format!("Cannot restore terminal: {error}"))?;
    let _ = CAPABILITIES.set(capabilities);
    requested.verified()
}

#[cfg(not(unix))]
pub fn verify_terminal(_: GraphicsMode) -> Result<GraphicsMode, String> {
    Err("Kitty identity detection currently requires a Unix terminal. Use the Kitty terminal emulator on Linux with any shell.".into())
}

fn parse_capabilities(response: &[u8]) -> Capabilities {
    let response = String::from_utf8_lossy(response);
    let graphics = response.contains("\x1b_Gi=48391;OK\x1b\\");
    let kitty_terminal = response
        .split("\x1bP>|")
        .skip(1)
        .filter_map(|part| part.split_once("\x1b\\").map(|(identity, _)| identity))
        .any(|identity| identity.starts_with("kitty(") && identity.ends_with(')'));
    Capabilities {
        kitty_terminal,
        graphics,
    }
}

pub fn kitty_image(
    rgb: &[u8],
    width: u32,
    height: u32,
    columns: u16,
    rows: u16,
    image_id: u32,
) -> Vec<u8> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("in-memory PNG header");
        writer.write_image_data(rgb).expect("in-memory PNG data");
    }
    let encoded = base64(&png);
    let mut output = Vec::with_capacity(encoded.len() + encoded.len() / 4096 * 32 + 128);
    for (index, chunk) in encoded.as_bytes().chunks(4096).enumerate() {
        let more = usize::from((index + 1) * 4096 < encoded.len());
        if index == 0 {
            output.extend_from_slice(
                format!(
                    "\x1b_Ga=T,f=100,c={columns},r={rows},i={image_id},p=1,z=-1,q=2,C=1,m={more};"
                )
                .as_bytes(),
            );
        } else {
            output.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        output.extend_from_slice(chunk);
        output.extend_from_slice(b"\x1b\\");
    }
    output
}

pub fn kitty_delete(image_id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\").into_bytes()
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_successful_graphics_responses_are_accepted() {
        assert!(parse_capabilities(b"\x1b_Gi=48391;OK\x1b\\\x1b[?62;1;2c").graphics);
        assert!(!parse_capabilities(b"\x1b_Gi=48391;EINVAL\x1b\\\x1b[?62;1;2c").graphics);
        assert!(!parse_capabilities(b"\x1b_Gi=999;OK\x1b\\").graphics);
        assert!(parse_capabilities(b"\x1bP>|kitty(0.42.0)\x1b\\").kitty_terminal);
        assert!(!parse_capabilities(b"\x1bP>|WezTerm(20261001)\x1b\\").kitty_terminal);
        assert!(!parse_capabilities(b"\x1bP>|kitty-clone(1)\x1b\\").kitty_terminal);
        let unsupported = parse_capabilities(b"\x1b[?62;1;2;6;9;15;22c");
        assert!(!unsupported.kitty_terminal && !unsupported.graphics);
        assert!(GraphicsMode::parse("ascii").is_none());
        assert!(GraphicsMode::parse("text").is_none());
        assert!(GraphicsMode::parse("sixel").is_none());
    }

    #[test]
    fn kitty_payload_contains_geometry_and_data() {
        let payload = kitty_image(&[255, 0, 0], 1, 1, 1, 1, 42);
        let text = String::from_utf8(payload).unwrap();
        assert!(text.contains("f=100,c=1,r=1,i=42,p=1"));
        assert!(text.len() > 40);
    }
}
