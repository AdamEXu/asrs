//! The per-realm state upstream keeps in module globals.
//!
//! Every upstream JS realm (the hub renderer and each decode worker) has its own
//! `@msgpack/msgpack` key cache, protobufjs counters for unnamed descriptors and `Math.random`
//! stream. Logs in one realm share that state, so a caller keeps one [`JsRealm`] per upstream
//! realm and passes it to every `Log` operation that touches it.

use asrs_js::string::JsString;
use asrs_js::time::Rng;

use crate::log::proto_decoder::ProtoRealm;
use crate::msgpack::KeyCache;

/// What upstream's own code assigns to the realm's `Object.prototype`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectPrototype {
    /// `Object.prototype.fullKey`, which `getFieldTree` assigns for a key that ends at
    /// `__proto__`.
    pub full_key: Option<JsString>,
}

pub struct JsRealm {
    pub object_prototype: ObjectPrototype,
    pub msgpack_keys: KeyCache,
    pub proto: ProtoRealm,
    pub random: Box<dyn Rng + Send>,
}

impl JsRealm {
    pub fn new(random: Box<dyn Rng + Send>) -> Self {
        JsRealm {
            object_prototype: ObjectPrototype::default(),
            msgpack_keys: KeyCache::new(),
            proto: ProtoRealm::new(),
            random,
        }
    }
}
