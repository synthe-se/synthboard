//! The door into the native library, and the only `unsafe` in the project. Each
//! thread shares two buffers with Kotlin, outside the Java heap: the garbage
//! collector never copies them, and nothing here calls back into the JVM, which
//! would cost more than the rest of a call. The JNI function index is fixed by
//! the JNI specification.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::OnceLock;

use zeroize::Zeroize;

use crate::{Keyboard, handle};

type Env = *const *const *const c_void;
type Object = *mut c_void;
type GetAddress = unsafe extern "system" fn(Env, Object) -> *mut c_void;

const GET_DIRECT_BUFFER_ADDRESS: usize = 230;

thread_local! {
    static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Wipes the reply for the next call, and lets go of one that was huge.
fn finish(reply: &mut Vec<u8>) {
    reply.fill(0);
    reply.clear();
    if reply.capacity() > KEPT_BYTES {
        *reply = Vec::new();
    }
}

fn keyboard() -> &'static Keyboard {
    static KEYBOARD: OnceLock<Keyboard> = OnceLock::new();
    KEYBOARD.get_or_init(Keyboard::new)
}

/// A reply larger than this was the emoji catalogue, which comes once.
const KEPT_BYTES: usize = 64 * 1024;

/// Where a direct buffer lives. Kotlin asks once per buffer.
///
/// # Safety
/// Only the JVM calls this, with its environment and a direct buffer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_app_synthboard_core_Native_address(env: Env, _class: Object, buffer: Object) -> i64 {
    // SAFETY: the table has this entry, of this type, as the specification fixes it.
    unsafe {
        let function = (*env).add(GET_DIRECT_BUFFER_ADDRESS).cast::<GetAddress>().read();
        function(env, buffer) as i64
    }
}

/// Answers the call in the `size` bytes at `input`, and wipes them. Returns the
/// length of the reply, which is at `output` if it fits in `capacity`. If not,
/// the caller gives a larger buffer to `take`.
///
/// # Safety
/// Only Kotlin calls this, with live buffers it owns, on this thread, of these sizes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_app_synthboard_core_Native_call(
    _env: Env,
    _class: Object,
    input: i64,
    size: i32,
    output: i64,
    capacity: i32,
) -> i32 {
    // SAFETY: Kotlin keeps this buffer alive, and leaves it alone, during the call.
    let bytes = unsafe { std::slice::from_raw_parts_mut(input as *mut u8, usize::try_from(size).unwrap_or(0)) };
    REPLY.with_borrow_mut(|reply| {
        handle(keyboard(), bytes, reply);
        bytes.fill(0);
        let length = i32::try_from(reply.len()).unwrap_or(i32::MAX);
        if length <= capacity {
            // SAFETY: the reply fits in the output buffer, which Rust does not otherwise touch.
            unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), output as *mut u8, reply.len()) };
            finish(reply);
        }
        length
    })
}

/// The word the gesture under way would give, at `output`. It runs for every
/// frame of a gesture, so it skips messages altogether. Returns its length, or
/// 0 if there is none or it does not fit.
///
/// # Safety
/// Only Kotlin calls this, with a live buffer it owns, of this size.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_app_synthboard_core_Native_preview(
    _env: Env,
    _class: Object,
    output: i64,
    capacity: i32,
) -> i32 {
    let mut word = keyboard().preview();
    let length = i32::try_from(word.len()).unwrap_or(i32::MAX);
    let fits = length <= capacity;
    if fits {
        // SAFETY: the word fits in the output buffer.
        unsafe { std::ptr::copy_nonoverlapping(word.as_ptr(), output as *mut u8, word.len()) };
    }
    word.zeroize();
    if fits { length } else { 0 }
}

/// Hands over a reply that did not fit.
///
/// # Safety
/// Only Kotlin calls this, on the same thread, with a buffer of the length `call` returned.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_app_synthboard_core_Native_take(_env: Env, _class: Object, output: i64) {
    REPLY.with_borrow_mut(|reply| {
        // SAFETY: Kotlin sized this buffer to the reply.
        unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), output as *mut u8, reply.len()) };
        finish(reply);
    });
}
