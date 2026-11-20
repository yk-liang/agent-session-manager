use std::io::{Error, ErrorKind};

#[derive(Clone, Copy)]
enum Field<'a> {
    Number(u64),
    Bytes(&'a [u8]),
}

pub(crate) struct Message<'a>(Vec<(u32, Field<'a>)>);

fn invalid() -> Error {
    Error::new(ErrorKind::InvalidData, "Antigravity protobuf 数据无效")
}

fn varint(input: &mut &[u8]) -> std::io::Result<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input.split_first().ok_or_else(invalid)?;
        *input = rest;
        if shift == 63 && byte > 1 {
            return Err(invalid());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(invalid())
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> std::io::Result<&'a [u8]> {
    let (field, rest) = input.split_at_checked(len).ok_or_else(invalid)?;
    *input = rest;
    Ok(field)
}

impl<'a> Message<'a> {
    pub(crate) fn decode(mut input: &'a [u8]) -> std::io::Result<Self> {
        let mut fields = Vec::new();
        while !input.is_empty() {
            let tag = varint(&mut input)?;
            let number = u32::try_from(tag >> 3).map_err(|_| invalid())?;
            if number == 0 || number >= 1 << 29 {
                return Err(invalid());
            }
            let field = match tag & 7 {
                0 => Field::Number(varint(&mut input)?),
                2 => {
                    let size = usize::try_from(varint(&mut input)?).map_err(|_| invalid())?;
                    Field::Bytes(take(&mut input, size)?)
                }
                1 => {
                    take(&mut input, 8)?;
                    continue;
                }
                5 => {
                    take(&mut input, 4)?;
                    continue;
                }
                _ => return Err(invalid()),
            };
            fields.push((number, field));
        }
        Ok(Self(fields))
    }

    fn fields(&self, number: u32) -> impl DoubleEndedIterator<Item = Field<'a>> + '_ {
        self.0
            .iter()
            .filter_map(move |&(key, value)| (key == number).then_some(value))
    }

    pub(crate) fn number(&self, number: u32) -> Option<u64> {
        self.fields(number).rev().find_map(|field| match field {
            Field::Number(value) => Some(value),
            _ => None,
        })
    }

    pub(crate) fn repeated(&self, number: u32) -> impl DoubleEndedIterator<Item = &'a [u8]> + '_ {
        self.fields(number).filter_map(|field| match field {
            Field::Bytes(bytes) => Some(bytes),
            _ => None,
        })
    }

    pub(crate) fn text(&self, number: u32) -> Option<&'a str> {
        std::str::from_utf8(self.repeated(number).next_back()?)
            .ok()
            .filter(|text| !text.trim().is_empty())
    }

    pub(crate) fn message(&self, number: u32) -> std::io::Result<Self> {
        Self::decode(self.repeated(number).next_back().unwrap_or_default())
    }

    pub(crate) fn indices(&self, number: u32) -> std::io::Result<Vec<i64>> {
        let mut values = Vec::new();
        for field in self.fields(number) {
            match field {
                Field::Number(value) => values.push(i64::try_from(value).map_err(|_| invalid())?),
                Field::Bytes(mut packed) => {
                    while !packed.is_empty() {
                        values.push(i64::try_from(varint(&mut packed)?).map_err(|_| invalid())?);
                    }
                }
            }
        }
        Ok(values)
    }

    pub(crate) fn millis(&self, number: u32) -> std::io::Result<Option<i64>> {
        let time = self.message(number)?;
        let secs = time.number(1).unwrap_or_default();
        let nanos = time.number(2).unwrap_or_default();
        if secs == 0 && nanos == 0 && self.repeated(number).next_back().is_none() {
            return Ok(None);
        }
        let millis = i64::try_from(secs)
            .ok()
            .and_then(|secs| secs.checked_mul(1000))
            .and_then(|millis| millis.checked_add((nanos.min(999_999_999) / 1_000_000) as i64));
        Ok(millis)
    }
}
