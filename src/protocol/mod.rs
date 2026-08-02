pub(crate) mod crypto;
pub(crate) mod frame;
pub(crate) mod handshake;
pub(crate) mod wifi_handshake;

#[allow(clippy::enum_variant_names)]
pub(crate) mod proto {
    include!(concat!(env!("OUT_DIR"), "/smartsync.rs"));
}
