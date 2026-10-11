//! A minimal but complete NBT (Named Binary Tag) reader/writer.
//!
//! The original Java launcher parsed servers.dat with hand-rolled byte reads
//! and **dropped every unknown tag on rewrite** — which silently deleted the
//! per-server icons every time the user edited the list. This port parses the
//! full tree into a generic [`Value`], so the server list editor can preserve
//! everything it does not touch.

use std::fmt::Write as _;

use anyhow::{anyhow, bail, Result};

/// Maximum compound/list nesting we accept; real files never exceed a few levels.
const MAX_DEPTH: u32 = 64;

/// A generic NBT value. Compounds keep insertion order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Value>),
    Compound(Vec<(String, Value)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Value {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn compound_get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, n: usize) -> Result<()> {
        if self.pos + n > self.data.len() {
            bail!(
                "unexpected end of NBT data (need {n} bytes at offset {} of {})",
                self.pos,
                self.data.len()
            );
        }
        Ok(())
    }

    fn u8(&mut self) -> Result<u8> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| anyhow!("invalid UTF-8 in string: {e}"))
    }

    fn payload(&mut self, tag: u8, depth: u32) -> Result<Value> {
        if depth > MAX_DEPTH {
            bail!("NBT nesting deeper than {MAX_DEPTH}");
        }
        Ok(match tag {
            1 => Value::Byte(self.u8()? as i8),
            2 => Value::Short(self.i16()?),
            3 => Value::Int(self.i32()?),
            4 => Value::Long(self.i64()?),
            5 => Value::Float(f32::from_be_bytes(self.take(4)?.try_into().unwrap())),
            6 => Value::Double(f64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            7 => {
                let len = self.i32()?;
                let len = usize::try_from(len).map_err(|_| anyhow!("negative byte array len"))?;
                if len > 4 * 1024 * 1024 {
                    bail!("byte array too large: {len}");
                }
                Value::ByteArray(self.take(len)?.to_vec())
            }
            8 => Value::String(self.string()?),
            9 => {
                let inner = self.u8()?;
                let len = self.i32()?;
                let len = usize::try_from(len).map_err(|_| anyhow!("negative list len"))?;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.payload(inner, depth + 1)?);
                }
                Value::List(items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let t = self.u8()?;
                    if t == 0 {
                        break;
                    }
                    let name = self.string()?;
                    let value = self.payload(t, depth + 1)?;
                    entries.push((name, value));
                }
                Value::Compound(entries)
            }
            11 => {
                let len = self.i32()?;
                let len = usize::try_from(len).map_err(|_| anyhow!("negative int array len"))?;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.i32()?);
                }
                Value::IntArray(items)
            }
            12 => {
                let len = self.i32()?;
                let len = usize::try_from(len).map_err(|_| anyhow!("negative long array len"))?;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.i64()?);
                }
                Value::LongArray(items)
            }
            other => bail!("unknown NBT tag id {other}"),
        })
    }
}

/// Parse a whole NBT document: `(root name, root payload)`.
pub fn parse(data: &[u8]) -> Result<(String, Value)> {
    let mut r = Reader { data, pos: 0 };
    let tag = r.u8()?;
    if tag != 10 {
        bail!("NBT root must be a compound tag, got id {tag}");
    }
    let name = r.string()?;
    let value = r.payload(tag, 0)?;
    Ok((name, value))
}

struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }

    fn string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.u16(bytes.len() as u16);
        self.out.extend_from_slice(bytes);
    }

    fn payload(&mut self, value: &Value) {
        match value {
            Value::Byte(v) => self.u8(*v as u8),
            Value::Short(v) => self.out.extend_from_slice(&v.to_be_bytes()),
            Value::Int(v) => self.out.extend_from_slice(&v.to_be_bytes()),
            Value::Long(v) => self.out.extend_from_slice(&v.to_be_bytes()),
            Value::Float(v) => self.out.extend_from_slice(&v.to_be_bytes()),
            Value::Double(v) => self.out.extend_from_slice(&v.to_be_bytes()),
            Value::ByteArray(bytes) => {
                self.i32(bytes.len() as i32);
                self.out.extend_from_slice(bytes);
            }
            Value::String(s) => self.string(s),
            Value::List(items) => {
                let inner = items.first().map(tag_of).unwrap_or(0);
                self.u8(inner);
                self.i32(items.len() as i32);
                for item in items {
                    self.payload(item);
                }
            }
            Value::Compound(entries) => {
                for (name, value) in entries {
                    self.u8(tag_of(value));
                    self.string(name);
                    self.payload(value);
                }
                self.u8(0);
            }
            Value::IntArray(items) => {
                self.i32(items.len() as i32);
                for item in items {
                    self.out.extend_from_slice(&item.to_be_bytes());
                }
            }
            Value::LongArray(items) => {
                self.i32(items.len() as i32);
                for item in items {
                    self.out.extend_from_slice(&item.to_be_bytes());
                }
            }
        }
    }
}

/// The NBT tag id of a value (as written before the name).
pub fn tag_of(value: &Value) -> u8 {
    match value {
        Value::Byte(_) => 1,
        Value::Short(_) => 2,
        Value::Int(_) => 3,
        Value::Long(_) => 4,
        Value::Float(_) => 5,
        Value::Double(_) => 6,
        Value::ByteArray(_) => 7,
        Value::String(_) => 8,
        Value::List(_) => 9,
        Value::Compound(_) => 10,
        Value::IntArray(_) => 11,
        Value::LongArray(_) => 12,
    }
}

/// Serialize a NBT document with the given root name and payload.
pub fn write(root_name: &str, value: &Value) -> Vec<u8> {
    let mut w = Writer { out: Vec::new() };
    w.u8(10);
    w.string(root_name);
    w.payload(value);
    w.out
}

/// Compact debug dump for diagnostics output.
#[allow(dead_code)]
pub fn summary(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Compound(entries) => {
            for (name, v) in entries {
                let _ = writeln!(out, "{name}: {}", summary(v));
            }
        }
        Value::List(items) => {
            let _ = write!(out, "list[{}]", items.len());
        }
        other => {
            let _ = write!(out, "{other:?}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: &Value) -> Value {
        let bytes = write("", value);
        let (_, parsed) = parse(&bytes).unwrap();
        parsed
    }

    #[test]
    fn compound_roundtrip_all_types() {
        let value = Value::Compound(vec![
            ("b".into(), Value::Byte(-1)),
            ("s".into(), Value::Short(300)),
            ("i".into(), Value::Int(70_000)),
            ("l".into(), Value::Long(i64::MAX)),
            ("f".into(), Value::Float(1.5)),
            ("d".into(), Value::Double(-0.25)),
            ("bytes".into(), Value::ByteArray(vec![1, 2, 3])),
            ("str".into(), Value::String("привет".into())),
            (
                "list".into(),
                Value::List(vec![Value::Int(1), Value::Int(2)]),
            ),
            (
                "nested".into(),
                Value::Compound(vec![("inner".into(), Value::String("x".into()))]),
            ),
            ("ints".into(), Value::IntArray(vec![1, -2, 3])),
            ("longs".into(), Value::LongArray(vec![9, 8])),
        ]);
        assert_eq!(roundtrip(&value), value);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let bytes = write(
            "",
            &Value::Compound(vec![("s".into(), Value::String("hello".into()))]),
        );
        for cut in [1, 3, 10, bytes.len() - 1] {
            assert!(parse(&bytes[..cut]).is_err(), "cut at {cut} must fail");
        }
    }

    #[test]
    fn deep_nesting_is_rejected() {
        // 100 nested compounds would overflow the stack without the guard.
        let mut value = Value::Compound(vec![]);
        for _ in 0..100 {
            value = Value::Compound(vec![("n".into(), value)]);
        }
        let bytes = write("", &value);
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn servers_dat_shape_roundtrip() {
        // The exact shape Minecraft writes for two servers.
        let servers = Value::Compound(vec![(
            "servers".into(),
            Value::List(vec![
                Value::Compound(vec![
                    ("name".into(), Value::String("Hypixel".into())),
                    ("ip".into(), Value::String("mc.hypixel.net".into())),
                    (
                        "icon".into(),
                        Value::ByteArray(vec![0x89, b'P', b'N', b'G']),
                    ),
                ]),
                Value::Compound(vec![
                    ("name".into(), Value::String("Local".into())),
                    ("ip".into(), Value::String("127.0.0.1:25566".into())),
                ]),
            ]),
        )]);
        let parsed = roundtrip(&servers);
        let list = parsed.compound_get("servers").unwrap();
        let Value::List(items) = list else {
            panic!("servers must be a list");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].compound_get("icon").unwrap(),
            &Value::ByteArray(vec![0x89, b'P', b'N', b'G'])
        );
        assert_eq!(
            items[1].compound_get("ip").unwrap().as_string().unwrap(),
            "127.0.0.1:25566"
        );
    }

    #[test]
    fn empty_list_uses_end_tag() {
        let value = Value::Compound(vec![("empty".into(), Value::List(vec![]))]);
        let bytes = write("", &value);
        // Trailing part must be: 0 (end of root compound).
        assert_eq!(*bytes.last().unwrap(), 0);
        assert_eq!(roundtrip(&value), value);
    }
}
