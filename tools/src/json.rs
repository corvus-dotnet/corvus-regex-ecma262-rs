//! A small JSON reader whose strings are UTF-16, so that a lone surrogate written as `\ud801` is kept, as the
//! records of the ECMA-262 test data need (a JSON library refuses them or writes U+FFFD in their place).

/// A JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    /// A string, as UTF-16 code units.
    String(Vec<u16>),
    Array(Vec<Json>),
    /// An object, its members in order.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The member of an object.
    pub fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_units(&self) -> Option<&[u16]> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    /// The string, with a lone surrogate written as U+FFFD.
    pub fn as_string(&self) -> Option<String> {
        self.as_units().map(String::from_utf16_lossy)
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
}

/// Reads one JSON value, which is all of `text` but white space.
pub fn parse(text: &str) -> Result<Json, String> {
    let mut r = Reader { s: text.as_bytes(), i: 0 };
    let v = r.value()?;
    r.space();
    if r.i != r.s.len() {
        return Err(format!("unexpected text at {}", r.i));
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.space();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at {}", c as char, self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.space();
                    let name = String::from_utf16_lossy(&self.string()?);
                    self.eat(b':')?;
                    members.push((name, self.value()?));
                    self.space();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Object(members));
                        }
                        _ => return Err(format!("expected , or }} at {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.space();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(format!("expected , or ] at {}", self.i)),
                    }
                }
            }
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(_) => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    self.i += 1;
                }
                let n = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                n.parse::<f64>().map(Json::Number).map_err(|e| format!("{e} at {start}"))
            }
            None => Err("unexpected end".to_string()),
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("unexpected text at {}", self.i))
        }
    }

    fn string(&mut self) -> Result<Vec<u16>, String> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(format!("expected a string at {}", self.i));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return Err("unterminated string".to_string()) };
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4).ok_or("short \\u")?)
                                .map_err(|e| e.to_string())?;
                            out.push(u16::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
                            self.i += 4;
                        }
                        b'n' => out.push(0x0A),
                        b'r' => out.push(0x0D),
                        b't' => out.push(0x09),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        other => out.push(other as u16),
                    }
                }
                _ => {
                    // A run of UTF-8 up to the next quote or backslash.
                    let start = self.i - 1;
                    while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                        self.i += 1;
                    }
                    let run = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                    out.extend(run.encode_utf16());
                }
            }
        }
    }
}

/// A string as the test data writes it: JSON, ASCII, every other character a `\u` escape in lower case.
pub fn escape(units: &[u16]) -> String {
    let mut out = String::new();
    for &u in units {
        if (0x20..0x7f).contains(&u) && u != u16::from(b'\\') {
            out.push(u as u8 as char);
        } else {
            out.push_str(&format!("\\u{u:04x}"));
        }
    }
    out
}
