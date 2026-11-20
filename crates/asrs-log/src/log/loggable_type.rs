/// `shared/log/LoggableType.ts`. The numeric values are serialized
/// (`LogField.toSerialized` writes `type` as the enum index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LoggableType {
    Raw = 0,
    Boolean = 1,
    Number = 2,
    String = 3,
    BooleanArray = 4,
    NumberArray = 5,
    StringArray = 6,
    /// Placeholder for child fields of structured data; holds no samples.
    Empty = 7,
}
