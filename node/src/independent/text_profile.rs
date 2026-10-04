//! Released text_crdt.profile/v1 JSON edit shape, without changing payload bytes.
use std::collections::BTreeMap;
enum Value {
    String(Vec<u16>),
    Null,
    Other,
}
struct Parser<'a> {
    text: &'a str,
    index: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.index)
            .is_some_and(|c| matches!(c, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.index += 1;
        }
    }
    fn token(&mut self, token: u8) -> Option<()> {
        self.space();
        if *self.text.as_bytes().get(self.index)? != token {
            return None;
        }
        self.index += 1;
        Some(())
    }
    fn string(&mut self) -> Option<Vec<u16>> {
        self.token(b'"')?;
        let mut value = vec![];
        loop {
            let character = self.text.get(self.index..)?.chars().next()?;
            self.index += character.len_utf8();
            match character {
                '"' => return Some(value),
                '\\' => {
                    let escape = self.text.get(self.index..)?.chars().next()?;
                    self.index += escape.len_utf8();
                    match escape {
                        '"' => value.push(34),
                        '\\' => value.push(92),
                        '/' => value.push(47),
                        'b' => value.push(8),
                        'f' => value.push(12),
                        'n' => value.push(10),
                        'r' => value.push(13),
                        't' => value.push(9),
                        'u' => {
                            let hex = self.text.get(self.index..self.index.checked_add(4)?)?;
                            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                                return None;
                            }
                            value.push(u16::from_str_radix(hex, 16).ok()?);
                            self.index += 4;
                        }
                        _ => return None,
                    }
                }
                c if c < ' ' => return None,
                c => {
                    let mut units = [0; 2];
                    value.extend_from_slice(c.encode_utf16(&mut units));
                }
            }
        }
    }
    fn number(&mut self) -> Option<()> {
        self.space();
        let bytes = self.text.as_bytes();
        if bytes.get(self.index) == Some(&b'-') {
            self.index += 1;
        }
        match bytes.get(self.index)? {
            b'0' => self.index += 1,
            b'1'..=b'9' => {
                self.index += 1;
                while bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
                    self.index += 1;
                }
            }
            _ => return None,
        }
        if bytes.get(self.index) == Some(&b'.') {
            self.index += 1;
            let first = self.index;
            while bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
                self.index += 1;
            }
            if self.index == first {
                return None;
            }
        }
        if bytes
            .get(self.index)
            .is_some_and(|c| matches!(c, b'e' | b'E'))
        {
            self.index += 1;
            if bytes
                .get(self.index)
                .is_some_and(|c| matches!(c, b'+' | b'-'))
            {
                self.index += 1;
            }
            let first = self.index;
            while bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
                self.index += 1;
            }
            if self.index == first {
                return None;
            }
        }
        Some(())
    }
    // Atom consumes a primitive or opening bracket. The explicit bounded stack
    // below validates overwritten composite values without recursive Rust calls.
    fn atom(&mut self) -> Option<Option<u8>> {
        self.space();
        let byte = *self.text.as_bytes().get(self.index)?;
        if byte == b'[' || byte == b'{' {
            self.index += 1;
            return Some(Some(byte));
        }
        if byte == b'"' {
            self.string()?;
            return Some(None);
        }
        for literal in ["null", "true", "false"] {
            if self.text.get(self.index..)?.starts_with(literal) {
                self.index += literal.len();
                return Some(None);
            }
        }
        self.number()?;
        Some(None)
    }
    fn composite(&mut self, opening: u8) -> Option<()> {
        let mut stack = vec![(opening, 0u8)];
        while let Some((opening, state)) = stack.last().copied() {
            self.space();
            let closing = if opening == b'[' { b']' } else { b'}' };
            let byte = *self.text.as_bytes().get(self.index)?;
            if state == 2 {
                if byte == closing {
                    self.index += 1;
                    stack.pop();
                    continue;
                }
                self.token(b',')?;
                stack.last_mut()?.1 = 1;
                continue;
            }
            if state == 0 && byte == closing {
                self.index += 1;
                stack.pop();
                continue;
            }
            if opening == b'{' {
                self.string()?;
                self.token(b':')?;
            }
            stack.last_mut()?.1 = 2;
            if let Some(child) = self.atom()? {
                stack.push((child, 0));
            }
        }
        Some(())
    }
    fn value(&mut self) -> Option<Value> {
        self.space();
        if self.text.as_bytes().get(self.index) == Some(&b'"') {
            return Some(Value::String(self.string()?));
        }
        if self.text.get(self.index..)?.starts_with("null") {
            self.index += 4;
            return Some(Value::Null);
        }
        if let Some(opening) = self.atom()? {
            self.composite(opening)?;
        }
        Some(Value::Other)
    }
    fn object(&mut self) -> Option<BTreeMap<String, Value>> {
        self.token(b'{')?;
        let mut fields = BTreeMap::new();
        self.space();
        if self.text.as_bytes().get(self.index) == Some(&b'}') {
            self.index += 1;
            return Some(fields);
        }
        loop {
            let key = String::from_utf16(&self.string()?).ok()?;
            self.token(b':')?;
            let value = self.value()?;
            fields.insert(key, value);
            self.space();
            let separator = *self.text.as_bytes().get(self.index)?;
            self.index += 1;
            if separator == b'}' {
                break;
            }
            if separator != b',' {
                return None;
            }
        }
        self.space();
        if self.index != self.text.len() {
            return None;
        }
        Some(fields)
    }
}
fn string<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> Option<&'a [u16]> {
    match fields.get(name)? {
        Value::String(value) => Some(value),
        Value::Null | Value::Other => None,
    }
}
pub fn valid_text_payload(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some(fields) = (Parser { text, index: 0 }).object() else {
        return false;
    };
    let Some(kind) = string(&fields, "kind") else {
        return false;
    };
    let Some(atom) = string(&fields, "atom_id") else {
        return false;
    };
    if atom.is_empty() {
        return false;
    }
    if kind == "delete".encode_utf16().collect::<Vec<_>>() {
        return fields.len() == 2;
    }
    kind == "insert".encode_utf16().collect::<Vec<_>>()
        && fields.len() == 4
        && string(&fields, "text").is_some_and(|text| !text.is_empty())
        && matches!(fields.get("after"), Some(Value::Null | Value::String(_)))
}
