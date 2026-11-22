//! `@rev-robotics/revlog-converter` 2.0.0 `dist/lib/revDBC.js`: the `Dbc` line parser and the
//! `CanDecoder` that turns a CAN frame into signal values. Numbers stay `f64` where upstream
//! keeps a JS number (ids, bit positions and lengths come from `parseInt`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Int,
    Float,
    Double,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub start_bit: f64,
    pub length: f64,
    pub is_little_endian: bool,
    pub is_signed: bool,
    pub factor: f64,
    pub offset: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub data_type: DataType,
    /// Set by a `CM_ SG_` line; upstream adds the `description` key only then.
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: f64,
    pub name: String,
    pub dlc: f64,
    pub sender: String,
    /// `signals`, a `Map` keyed by name, in its iteration order.
    pub signals: Vec<Signal>,
}

#[derive(Clone, Debug, Default)]
pub struct Dbc {
    _todo: (),
}

impl Dbc {
    /// `new Dbc().load(content)`.
    pub fn load(content: &str) -> Dbc {
        let _ = content;
        todo!("F14: port Dbc.load")
    }

    /// `dbc.messages`: name to message, in `Map` iteration order.
    pub fn messages(&self) -> Vec<(&str, &Message)> {
        todo!("F14: port Dbc.messages")
    }

    /// `dbc.messagesById`: id to message, in `Map` iteration order.
    pub fn messages_by_id(&self) -> Vec<(f64, &Message)> {
        todo!("F14: port Dbc.messagesById")
    }
}

/// A decoded signal value: a JS number, or a BigInt where upstream keeps one (an unscaled
/// integer outside the safe-integer range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalValue {
    Number(f64),
    BigInt(i128),
}

/// `CanDecoder` with `database` set.
pub struct CanDecoder<'a> {
    database: &'a Dbc,
}

impl<'a> CanDecoder<'a> {
    pub fn new(database: &'a Dbc) -> CanDecoder<'a> {
        CanDecoder { database }
    }

    /// `decode(createFrame(id, data))`: `None` for an id the database does not have, else
    /// each signal's `{ name, value }` in the message's signal order.
    pub fn decode(&self, id: f64, data: &[u8]) -> Option<Vec<(String, SignalValue)>> {
        let _ = (self.database, id, data);
        todo!("F14: port CanDecoder.decode")
    }
}
