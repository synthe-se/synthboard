//! Runs the model with ONNX Runtime. The static library comes from
//! `tools/build-ort.sh`: minimal, no network, no telemetry, the CPU only, and
//! only the kernels this model uses.

#![forbid(unsafe_code)]

use std::fmt;
use std::sync::{Mutex, OnceLock};

use ort::environment::Environment;
use ort::ep::CPU;
use ort::session::Session;
use ort::session::builder::AutoDevicePolicy;
use ort::value::TensorRef;
use synthswipe::{Encoder, FEATURES, FRAMES, Neural, split_model};

/// A process may only ever have one.
static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

const BATCH: usize = 1;
const FORMAT_KEY: &str = "session.load_model_format";
const FORMAT_ORT: &str = "ORT";

/// A loaded model. One gesture runs at a time, on the caller's thread.
pub struct Onnx {
    session: Mutex<Session>,
}

impl fmt::Debug for Onnx {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Onnx")
    }
}

impl Onnx {
    /// Loads a model in ONNX Runtime's own format.
    ///
    /// # Errors
    /// Returns the runtime's message if it refuses the bytes.
    pub fn new(model: &[u8]) -> Result<Self, String> {
        fn text(error: impl fmt::Display) -> String {
            error.to_string()
        }
        if ENVIRONMENT.get().is_none() {
            let _ = ENVIRONMENT.set(ort::init().with_telemetry(false).build().map_err(text)?);
        }
        let environment = ENVIRONMENT.get().ok_or("no environment")?;
        let session = Session::builder(environment)
            .map_err(text)?
            // Without this, macOS silently takes CoreML.
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(text)?
            .with_no_environment_execution_providers()
            .map_err(text)?
            .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
            .map_err(text)?
            // No thread of its own: a run belongs to the decoding thread.
            .with_intra_threads(1)
            .map_err(text)?
            .with_inter_threads(1)
            .map_err(text)?
            .with_parallel_execution(false)
            .map_err(text)?
            .with_intra_op_spinning(false)
            .map_err(text)?
            .with_inter_op_spinning(false)
            .map_err(text)?
            // Blocks are freed after each run, so that `wipe.rs` clears them.
            .with_memory_pattern(false)
            .map_err(text)?
            .with_config_entry(FORMAT_KEY, FORMAT_ORT)
            .map_err(text)?
            .commit_from_memory(model)
            .map_err(text)?;
        Ok(Self { session: Mutex::new(session) })
    }
}

impl Encoder for Onnx {
    fn encode(&self, input: &[f32], output: &mut [f32]) -> bool {
        let Ok(mut session) = self.session.lock() else {
            return false;
        };
        let Ok(tensor) = TensorRef::from_array_view(([BATCH, FEATURES, FRAMES], input)) else {
            return false;
        };
        let Ok(outputs) = session.run(ort::inputs![tensor]) else {
            return false;
        };
        let Some((_, value)) = outputs.iter().next() else {
            return false;
        };
        let Ok((_, values)) = value.try_extract_tensor::<f32>() else {
            return false;
        };
        if values.len() != output.len() {
            return false;
        }
        output.copy_from_slice(values);
        true
    }
}

/// Builds the decoder's model from a model file. `None` if the file is not one.
#[must_use]
pub fn load(bytes: &[u8]) -> Option<Neural> {
    let (head, model) = split_model(bytes)?;
    let encoder = Onnx::new(model).ok()?;
    Some(Neural::new(head, Box::new(encoder)))
}
