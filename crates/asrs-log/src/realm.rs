//! The per-realm state upstream keeps in module globals.
//!
//! Every upstream JS realm (the hub renderer and each decode worker) has its own
//! `@msgpack/msgpack` key cache, protobufjs counters for unnamed descriptors and `Math.random`
//! stream. Logs in one realm share that state, so a caller keeps one [`JsRealm`] per upstream
//! realm and passes it to every `Log` operation that touches it.

use asrs_js::time::Rng;

use crate::log::proto_decoder::ProtoRealm;
use crate::msgpack::KeyCache;

pub struct JsRealm {
    pub msgpack_keys: KeyCache,
    pub proto: ProtoRealm,
    pub random: Box<dyn Rng + Send>,
}

impl JsRealm {
    pub fn new(random: Box<dyn Rng + Send>) -> Self {
        JsRealm {
            msgpack_keys: KeyCache::new(),
            proto: ProtoRealm::new(),
            random,
        }
    }
}
