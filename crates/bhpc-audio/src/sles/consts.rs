//! OpenSL ES 1.0.1 / Android extension constants (values from the Khronos headers and the NDK).

// SLresult
pub const SL_RESULT_SUCCESS: u32 = 0;
pub const SL_RESULT_PRECONDITIONS_VIOLATED: u32 = 1;
pub const SL_RESULT_PARAMETER_INVALID: u32 = 2;
pub const SL_RESULT_MEMORY_FAILURE: u32 = 3;
pub const SL_RESULT_BUFFER_INSUFFICIENT: u32 = 7;
pub const SL_RESULT_CONTENT_UNSUPPORTED: u32 = 9;
pub const SL_RESULT_FEATURE_UNSUPPORTED: u32 = 12;

// SLObjectItf states
pub const SL_OBJECT_STATE_UNREALIZED: u32 = 1;
pub const SL_OBJECT_STATE_REALIZED: u32 = 2;
pub const SL_OBJECT_EVENT_ASYNC_TERMINATION: u32 = 1;

// SLPlayItf
pub const SL_PLAYSTATE_STOPPED: u32 = 1;
pub const SL_PLAYSTATE_PAUSED: u32 = 2;
pub const SL_PLAYSTATE_PLAYING: u32 = 3;
pub const SL_PLAYEVENT_HEADATEND: u32 = 1;
pub const SL_TIME_UNKNOWN: u32 = 0xFFFF_FFFF;

// Data locators and formats
pub const SL_DATALOCATOR_OUTPUTMIX: u32 = 4;
pub const SL_DATALOCATOR_BUFFERQUEUE: u32 = 6;
pub const SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE: u32 = 0x8000_07BD;
pub const SL_DATAFORMAT_PCM: u32 = 2;
pub const SL_ANDROID_DATAFORMAT_PCM_EX: u32 = 4;
pub const SL_BYTEORDER_LITTLEENDIAN: u32 = 2;
pub const SL_ANDROID_PCM_REPRESENTATION_UNSIGNED_INT: u32 = 2;
pub const SL_ANDROID_PCM_REPRESENTATION_FLOAT: u32 = 3;

/// The interface IDs the host knows, in export order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iid {
    Null,
    Object,
    Engine,
    Play,
    BufferQueue,
    AndroidSimpleBufferQueue,
    Volume,
    Record,
}

pub(crate) const fn uuid(l: u32, m: u16, h: u16, c: u16, node: [u8; 6]) -> [u8; 16] {
    let (l, m, h, c) = (l.to_le_bytes(), m.to_le_bytes(), h.to_le_bytes(), c.to_le_bytes());
    [l[0], l[1], l[2], l[3], m[0], m[1], h[0], h[1], c[0], c[1], node[0], node[1], node[2], node[3], node[4], node[5]]
}

const NODE: [u8; 6] = [0x00, 0x02, 0xa5, 0xd5, 0xc5, 0x1b];

/// `(exported symbol, which, 16-byte SLInterfaceID_ layout)`.
pub const IIDS: [(&str, Iid, [u8; 16]); 8] = [
    ("SL_IID_NULL", Iid::Null, uuid(0, 0, 0, 0, [0; 6])),
    ("SL_IID_OBJECT", Iid::Object, uuid(0x7921_6360, 0xddd7, 0x11db, 0xac16, NODE)),
    ("SL_IID_ENGINE", Iid::Engine, uuid(0x8d97_c260, 0xddd4, 0x11db, 0x958f, [0x00, 0x02, 0xa5, 0xd5, 0xc9, 0xb2])),
    ("SL_IID_PLAY", Iid::Play, uuid(0xef0b_d9c0, 0xddd7, 0x11db, 0xbf49, NODE)),
    ("SL_IID_BUFFERQUEUE", Iid::BufferQueue, uuid(0x2bc9_9cc0, 0xddd4, 0x11db, 0x8d99, NODE)),
    ("SL_IID_ANDROIDSIMPLEBUFFERQUEUE", Iid::AndroidSimpleBufferQueue, uuid(0x198e_4940, 0xc5d7, 0x11df, 0xa2a6, NODE)),
    ("SL_IID_VOLUME", Iid::Volume, uuid(0x09e8_ede0, 0xddd4, 0x11db, 0xa4ce, NODE)),
    ("SL_IID_RECORD", Iid::Record, uuid(0xc565_7aa0, 0xdddb, 0x11db, 0x82f7, NODE)),
];

/// Identify an interface ID by its bytes.
pub fn iid_of(bytes: &[u8; 16]) -> Option<Iid> {
    IIDS.iter().find(|(_, _, b)| b == bytes).map(|(_, i, _)| *i)
}
