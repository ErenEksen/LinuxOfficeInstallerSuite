//! Wire format of the Soda identity bridge socket.
//!
//! Every message is a 20-byte little-endian header (magic, version, opcode, request id,
//! payload size, status) followed by a field table: per field an id, zero flags and a size,
//! then the bytes. Responses carry the request opcode with [`RESPONSE_BIT`] set.

use std::collections::BTreeMap;
use std::io::{self, Read};

pub const MAGIC: u32 = 0x3142_4953;
pub const VERSION: u16 = 1;
pub const RESPONSE_BIT: u16 = 0x8000;
pub const MAX_PAYLOAD: usize = 1024 * 1024;
const MAX_FIELDS: usize = 64;
const HEADER_LEN: usize = 20;
const FIELD_LEN: usize = 8;

pub const OP_PING: u16 = 1;
pub const OP_AUTHORIZE: u16 = 2;
pub const OP_GET_TOKEN: u16 = 3;
pub const OP_CLEAR: u16 = 4;
pub const OP_LIST_ACCOUNTS: u16 = 5;

pub const STATUS_OK: i32 = 0;
pub const STATUS_CANCELED: i32 = 1;
pub const STATUS_INVALID_REQUEST: i32 = 3;
pub const STATUS_AUTH_FAILED: i32 = 4;
pub const STATUS_NOT_FOUND: i32 = 5;

pub const FIELD_CLIENT_ID: u16 = 1;
pub const FIELD_SCOPES: u16 = 2;
pub const FIELD_AUTHORITY: u16 = 3;
pub const FIELD_LOGIN_HINT: u16 = 4;
pub const FIELD_CALLER_ID: u16 = 5;
pub const FIELD_ACCESS_TOKEN: u16 = 10;
pub const FIELD_ID_TOKEN: u16 = 11;
pub const FIELD_EXPIRES_AT: u16 = 12;
pub const FIELD_ACCOUNT_ID: u16 = 13;
pub const FIELD_USERNAME: u16 = 14;
pub const FIELD_FIRST_NAME: u16 = 15;
pub const FIELD_LAST_NAME: u16 = 16;
pub const FIELD_DISPLAY_NAME: u16 = 17;
pub const FIELD_TENANT_ID: u16 = 18;
pub const FIELD_CLIENT_INFO: u16 = 19;
pub const FIELD_ERROR: u16 = 20;
pub const FIELD_POLICY: u16 = 21;
pub const FIELD_ACCOUNTS: u16 = 22;
pub const FIELD_PROMPT: u16 = 23;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub opcode: u16,
    pub request_id: u32,
    pub status: i32,
    pub fields: BTreeMap<u16, Vec<u8>>,
}

impl Message {
    /// A response to `request` with `status` and `fields`.
    pub fn reply(request: &Message, status: i32, fields: BTreeMap<u16, Vec<u8>>) -> Message {
        Message { opcode: request.opcode | RESPONSE_BIT, request_id: request.request_id, status, fields }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        for (id, value) in &self.fields {
            payload.extend_from_slice(&id.to_le_bytes());
            payload.extend_from_slice(&0u16.to_le_bytes());
            payload.extend_from_slice(&(value.len() as u32).to_le_bytes());
            payload.extend_from_slice(value);
        }
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.opcode.to_le_bytes());
        out.extend_from_slice(&self.request_id.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.status.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Read one message from a stream.
    pub fn read(r: &mut impl Read) -> Result<Message, ProtoError> {
        let mut h = [0u8; HEADER_LEN];
        r.read_exact(&mut h)?;
        let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        if u32_at(0) != MAGIC || u16_at(4) != VERSION {
            return Err(ProtoError::Invalid("unsupported protocol"));
        }
        let size = u32_at(12) as usize;
        if size > MAX_PAYLOAD {
            return Err(ProtoError::Invalid("payload is too large"));
        }
        let mut payload = vec![0u8; size];
        r.read_exact(&mut payload)?;
        let mut msg = Message {
            opcode: u16_at(6),
            request_id: u32_at(8),
            status: i32::from_le_bytes([h[16], h[17], h[18], h[19]]),
            fields: BTreeMap::new(),
        };
        let mut off = 0;
        while off < payload.len() {
            if msg.fields.len() == MAX_FIELDS || off + FIELD_LEN > payload.len() {
                return Err(ProtoError::Invalid("invalid field table"));
            }
            let p = &payload[off..];
            let id = u16::from_le_bytes([p[0], p[1]]);
            let flags = u16::from_le_bytes([p[2], p[3]]);
            let len = u32::from_le_bytes([p[4], p[5], p[6], p[7]]) as usize;
            off += FIELD_LEN;
            if flags != 0 || len > payload.len() - off || msg.fields.contains_key(&id) {
                return Err(ProtoError::Invalid("invalid field"));
            }
            msg.fields.insert(id, payload[off..off + len].to_vec());
            off += len;
        }
        Ok(msg)
    }

    /// A UTF-8 text field; missing fields are empty.
    pub fn text(&self, id: u16) -> Result<String, ProtoError> {
        match self.fields.get(&id) {
            None => Ok(String::new()),
            Some(v) => String::from_utf8(v.clone()).map_err(|_| ProtoError::Invalid("invalid UTF-8 field")),
        }
    }
}

/// Encode an account list for [`FIELD_ACCOUNTS`]: a count, then per account the id and the
/// user name, each as a length-prefixed string.
pub fn encode_accounts(accounts: &[(String, String)]) -> Vec<u8> {
    let mut out = (accounts.len() as u32).to_le_bytes().to_vec();
    for (id, name) in accounts {
        for s in [id, name] {
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_message() {
        let mut fields = BTreeMap::new();
        fields.insert(FIELD_CLIENT_ID, b"d3590ed6-52b3-4102-aeff-aad2292ab01c".to_vec());
        fields.insert(FIELD_SCOPES, b"openid profile".to_vec());
        let m = Message { opcode: OP_AUTHORIZE, request_id: 7, status: 0, fields };
        let bytes = m.encode();
        assert_eq!(&bytes[..4], &MAGIC.to_le_bytes());
        assert_eq!(Message::read(&mut bytes.as_slice()).unwrap(), m);
        assert_eq!(Message::read(&mut bytes.as_slice()).unwrap().text(FIELD_SCOPES).unwrap(), "openid profile");
    }

    #[test]
    fn rejects_bad_messages() {
        let ping = Message { opcode: OP_PING, ..Default::default() }.encode();
        let mut wrong_magic = ping.clone();
        wrong_magic[0] ^= 1;
        assert!(Message::read(&mut wrong_magic.as_slice()).is_err());
        assert!(Message::read(&mut &ping[..10]).is_err(), "short header");

        let mut fields = BTreeMap::new();
        fields.insert(1, b"x".to_vec());
        let mut flagged = Message { opcode: OP_PING, fields, ..Default::default() }.encode();
        flagged[HEADER_LEN + 2] = 1;
        assert!(Message::read(&mut flagged.as_slice()).is_err(), "non-zero field flags");
    }

    #[test]
    fn encodes_accounts() {
        let b = encode_accounts(&[("a".into(), "u@x".into())]);
        assert_eq!(b, [1, 0, 0, 0, 1, 0, 0, 0, b'a', 3, 0, 0, 0, b'u', b'@', b'x']);
    }
}
