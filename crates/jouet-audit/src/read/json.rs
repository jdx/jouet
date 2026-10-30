//! Lenient JSON pieces for transcripts.
//!
//! Transcript formats change between harness versions, so a field that turns
//! up with an unexpected type is read as absent instead of failing its line,
//! and large values the audit does not need are skipped without being kept.

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, de::value::MapAccessDeserializer};
use std::fmt;

/// Accepts every scalar a visitor has no use for as `$none`.
macro_rules! scalars {
    ($none:expr) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($none)
        }
    };
}

/// A string, or `None` for any other value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Str(pub(crate) Option<String>);

impl Str {
    pub(crate) fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<'de> Deserialize<'de> for Str {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Str;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Str, E> {
                Ok(Str(Some(v.to_string())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Str, E> {
                Ok(Str(Some(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Str, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Str(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Str, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Str(None))
            }
            scalars!(Str(None));
        }
        d.deserialize_any(V)
    }
}

/// A number, or `None` for any other value.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Num(pub(crate) Option<f64>);

impl Num {
    pub(crate) fn u64(self) -> Option<u64> {
        self.0.filter(|n| *n >= 0.0).map(|n| n as u64)
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Num;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Num, E> {
                Ok(Num(Some(v as f64)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Num, E> {
                Ok(Num(Some(v as f64)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Num, E> {
                Ok(Num(Some(v)))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_none<E: de::Error>(self) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Num, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Num(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Num, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Num(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// A `true`, or `false` for any other value.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Flag(pub(crate) bool);

impl<'de> Deserialize<'de> for Flag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Flag(matches!(
            serde_json::Value::deserialize(d),
            Ok(serde_json::Value::Bool(true))
        )))
    }
}

/// An object read as `T`, or `None` for any other value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Obj<T>(pub Option<T>);

impl<T> Default for Obj<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Obj<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Obj<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Obj<T>, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(|t| Obj(Some(t)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Obj<T>, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Obj(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Obj<T>, E> {
                Ok(Obj(None))
            }
            scalars!(Obj(None));
        }
        d.deserialize_any(V(std::marker::PhantomData))
    }
}

/// The last string of an array, or a lone string: the script of
/// `["/bin/zsh", "-lc", "cargo test"]`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LastStr(pub(crate) Option<String>);

impl<'de> Deserialize<'de> for LastStr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = LastStr;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<LastStr, E> {
                Ok(LastStr(Some(v.to_string())))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<LastStr, A::Error> {
                let mut last = None;
                while let Some(Str(s)) = seq.next_element::<Str>()? {
                    if s.is_some() {
                        last = s;
                    }
                }
                Ok(LastStr(last))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<LastStr, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(LastStr(None))
            }
            scalars!(LastStr(None));
        }
        d.deserialize_any(V)
    }
}

/// What a tool's output says about itself, without keeping the output: its
/// size, the start of its first text, and whether it reported a GitHub rate
/// limit. A string, an array of `{"text": ...}` blocks, or one such block.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Output {
    pub(crate) bytes: u64,
    pub(crate) head: String,
    pub(crate) rate_limited: bool,
}

/// How much of the first text is kept, for exit codes and wall times.
const HEAD: usize = 512;

impl Output {
    fn add(&mut self, text: &str) {
        self.bytes += text.len() as u64;
        if self.head.len() < HEAD {
            let mut end = (HEAD - self.head.len()).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.head.push_str(&text[..end]);
        }
        if !self.rate_limited {
            self.rate_limited = text.contains("rate limit exceeded")
                || text.contains("secondary rate limit")
                || text.contains("RATE_LIMITED");
        }
    }
}

#[derive(Deserialize)]
struct TextBlock {
    #[serde(default)]
    text: Str,
}

impl<'de> Deserialize<'de> for Output {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Output;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("tool output")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Output, E> {
                let mut out = Output::default();
                out.add(v);
                Ok(out)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Output, A::Error> {
                let mut out = Output::default();
                while let Some(Obj(block)) = seq.next_element::<Obj<TextBlock>>()? {
                    if let Some(text) = block.and_then(|b| b.text.0) {
                        out.add(&text);
                    }
                }
                Ok(out)
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Output, A::Error> {
                let block = TextBlock::deserialize(MapAccessDeserializer::new(map))?;
                let mut out = Output::default();
                if let Some(text) = block.text.0 {
                    out.add(&text);
                }
                Ok(out)
            }
            scalars!(Output::default());
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        s: Str,
        #[serde(default)]
        n: Num,
        #[serde(default)]
        o: Obj<TextBlock>,
        #[serde(default)]
        last: LastStr,
        #[serde(default)]
        out: Output,
        #[serde(default)]
        flag: Flag,
    }

    #[test]
    fn unexpected_types_read_as_absent() {
        let p: Probe = serde_json::from_str(
            r#"{"s":[1,2],"n":"x","o":"y","last":{"a":1},"out":null,"flag":"true"}"#,
        )
        .unwrap();
        assert_eq!(p.s.0, None);
        assert_eq!(p.n.0, None);
        assert!(p.o.0.is_none());
        assert_eq!(p.last.0, None);
        assert_eq!(p.out, Output::default());
        assert!(!p.flag.0);
    }

    #[test]
    fn reads_the_shapes_it_expects() {
        let p: Probe = serde_json::from_str(
            r#"{"s":"hi","n":12,"o":{"text":"t"},"last":["zsh","-lc","ls"],"flag":true,
                "out":[{"type":"text","text":"API rate limit exceeded"},{"type":"image"}]}"#,
        )
        .unwrap();
        assert_eq!(p.s.as_deref(), Some("hi"));
        assert_eq!(p.n.u64(), Some(12));
        assert_eq!(p.o.0.and_then(|b| b.text.0).as_deref(), Some("t"));
        assert_eq!(p.last.0.as_deref(), Some("ls"));
        assert!(p.flag.0);
        assert_eq!(p.out.bytes, 23);
        assert!(p.out.rate_limited);
    }
}
