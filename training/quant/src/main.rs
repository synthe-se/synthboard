//! Quantises the exported float graph to int8 and converts it to ONNX Runtime's format. All
//! files live in one model directory, the one `neural-train export` wrote to.

use ndarray::ArrayD;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::session::Session;
use ort::value::TensorRef;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use synthswipe::{FEATURES, FRAMES, INPUT_VALUES};

#[allow(clippy::all, dead_code)]
pub mod onnx {
    include!(concat!(env!("OUT_DIR"), "/onnx.rs"));
}
use onnx::{AttributeProto, ModelProto, NodeProto, TensorProto, TypeProto, ValueInfoProto, attribute_proto::AttributeType, tensor_proto::DataType, type_proto};

const USAGE: &str = "usage:
  neural-quant quant MODEL_DIR SOURCE.onnx TARGET.onnx [--input-tail SHARE]
  neural-quant run MODEL_DIR MODEL.onnx OUTPUT_PREFIX   (eval0.bin and on, for neural-train evalq)
  neural-quant convert MODEL_DIR SOURCE.onnx TARGET.ort";

/// One gesture, as the graph reads it.
const SHAPE: [usize; 3] = [1, FEATURES, FRAMES];
/// ONNX's default for BatchNormalization.
const BATCH_NORM_EPSILON: f32 = 1e-5;
/// Keeps a scale from zero on a tensor that is all zeros.
const SMALLEST_SCALE: f32 = 1e-9;
/// Asymmetric int8 for activations.
const INT8_STEPS: f32 = 255.0;
const INT8_LOW: f32 = -128.0;
const INT8_HIGH: f32 = 127.0;

fn env() -> &'static ort::environment::Environment {
    static ENVIRONMENT: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    ENVIRONMENT.get_or_init(|| ort::init().with_telemetry(false).build().expect("ONNX Runtime starts"))
}

fn le_floats(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks().0.iter().copied().map(f32::from_le_bytes).collect()
}

fn floats(path: &Path) -> Vec<f32> {
    le_floats(&std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

fn session(path: &Path) -> ort::Result<Session> {
    Session::builder(env())?
        .with_auto_device(AutoDevicePolicy::PreferCPU)?
        .with_intra_threads(1)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file(path)
}

fn tget(m: &ModelProto, n: &str) -> (Vec<i64>, Vec<f32>) {
    let graph = m.graph.as_ref().expect("the model has a graph");
    let t = graph.initializer.iter().find(|t| t.name == n).unwrap_or_else(|| panic!("no initializer {n}"));
    let values = if t.float_data.is_empty() { le_floats(&t.raw_data) } else { t.float_data.clone() };
    (t.dims.clone(), values)
}
fn tf(name: &str, dims: &[i64], d: &[f32]) -> TensorProto {
    TensorProto { name: name.into(), dims: dims.to_vec(), data_type: DataType::Float as i32, raw_data: d.iter().flat_map(|f| f.to_le_bytes()).collect(), ..Default::default() }
}
fn ti8(name: &str, dims: &[i64], d: &[i8]) -> TensorProto {
    TensorProto { name: name.into(), dims: dims.to_vec(), data_type: DataType::Int8 as i32, raw_data: d.iter().map(|&v| v as u8).collect(), ..Default::default() }
}
fn ti32(name: &str, dims: &[i64], d: &[i32]) -> TensorProto {
    TensorProto { name: name.into(), dims: dims.to_vec(), data_type: DataType::Int32 as i32, raw_data: d.iter().flat_map(|f| f.to_le_bytes()).collect(), ..Default::default() }
}
fn node(op: &str, name: String, input: Vec<String>, output: Vec<String>, axis: Option<i64>) -> NodeProto {
    let attribute = axis.map(|i| AttributeProto { name: "axis".into(), i, r#type: AttributeType::Int as i32, ..Default::default() });
    NodeProto { op_type: op.into(), name, input, output, attribute: attribute.into_iter().collect(), ..Default::default() }
}

/// Folds each BatchNormalization into the Conv feeding it, which leaves integer-friendly nodes.
fn fold_batch_norm(m: &mut ModelProto) -> usize {
    let g = m.graph.as_ref().expect("the model has a graph").clone();
    let (mut count, mut remap) = (0, HashMap::new());
    let mut replace: Vec<TensorProto> = vec![];
    let mut dropped = vec![];
    for n in g.node.iter().filter(|n| n.op_type == "BatchNormalization") {
        let conv = g.node.iter().find(|c| c.op_type == "Conv" && c.output[0] == n.input[0]).expect("a Conv feeds each norm");
        assert_eq!(g.node.iter().filter(|u| u.input.contains(&conv.output[0])).count(), 1, "the Conv has other consumers");
        let epsilon = n.attribute.iter().find(|a| a.name == "epsilon").map_or(BATCH_NORM_EPSILON, |a| a.f);
        let (gamma, beta, mean, variance) = (tget(m, &n.input[1]).1, tget(m, &n.input[2]).1, tget(m, &n.input[3]).1, tget(m, &n.input[4]).1);
        let (weight_dims, mut weights) = tget(m, &conv.input[1]);
        let (bias_dims, mut biases) = tget(m, &conv.input[2]);
        let per = weights.len() / weight_dims[0] as usize;
        for c in 0..weight_dims[0] as usize {
            let factor = gamma[c] / (variance[c] + epsilon).sqrt();
            for v in &mut weights[c * per..(c + 1) * per] {
                *v *= factor;
            }
            biases[c] = (biases[c] - mean[c]) * factor + beta[c];
        }
        replace.push(tf(&conv.input[1], &weight_dims, &weights));
        replace.push(tf(&conv.input[2], &bias_dims, &biases));
        remap.insert(n.output[0].clone(), conv.output[0].clone());
        dropped.push(n.name.clone());
        count += 1;
    }
    let graph = m.graph.as_mut().expect("the model has a graph");
    graph.node.retain(|n| !dropped.contains(&n.name));
    for n in &mut graph.node {
        for input in &mut n.input {
            if let Some(renamed) = remap.get(input) {
                input.clone_from(renamed);
            }
        }
    }
    for t in replace {
        if let Some(slot) = graph.initializer.iter_mut().find(|o| o.name == t.name) {
            *slot = t;
        }
    }
    let used: HashSet<String> = graph.node.iter().flat_map(|n| n.input.clone()).collect();
    graph.initializer.retain(|t| used.contains(&t.name));
    count
}

/// The range of every tensor over the calibration gestures. The input's may drop a share at each
/// end, `input_tail`, because a few extreme gestures would otherwise stretch its scale.
fn calibrate(dir: &Path, float_path: &Path, input_tail: f64) -> HashMap<String, (f32, f32)> {
    let bytes = std::fs::read(float_path).expect("the folded graph reads");
    let mut m = ModelProto::decode(&bytes[..]).expect("the folded graph decodes");
    let g = m.graph.as_mut().expect("the model has a graph");
    let existing: Vec<String> = g.output.iter().map(|o| o.name.clone()).collect();
    let names: Vec<String> = g.node.iter().flat_map(|n| n.output.clone()).filter(|o| !existing.contains(o)).collect();
    for n in &names {
        let tensor = type_proto::Tensor { elem_type: DataType::Float as i32, shape: None };
        g.output.push(ValueInfoProto { name: n.clone(), r#type: Some(TypeProto { value: Some(type_proto::Value::TensorType(tensor)), ..Default::default() }), ..Default::default() });
    }
    let observed = dir.join("_calibration.onnx");
    std::fs::write(&observed, m.encode_to_vec()).expect("the probe graph is written");
    let mut s = Session::builder(env()).expect("a builder")
        .with_auto_device(AutoDevicePolicy::PreferCPU).expect("the CPU")
        .with_optimization_level(GraphOptimizationLevel::Disable).expect("no rewrites")
        .commit_from_file(&observed).expect("the probe graph loads");
    let x = floats(&dir.join("calibration.bin"));
    let mut ranges: HashMap<String, (f32, f32)> = HashMap::new();
    for one in x.as_chunks::<INPUT_VALUES>().0 {
        let a = ArrayD::from_shape_vec(SHAPE.to_vec(), one.to_vec()).expect("one gesture");
        let view = TensorRef::from_array_view(a.view()).expect("a view");
        let out = s.run(ort::inputs![view]).expect("the graph runs");
        for (name, value) in out.iter() {
            let tensor = value.try_extract_tensor::<f32>().expect("a float tensor").1;
            let range = ranges.entry(name.to_string()).or_insert((f32::MAX, f32::MIN));
            for &v in tensor {
                range.0 = range.0.min(v);
                range.1 = range.1.max(v);
            }
        }
    }
    let mut sorted = x;
    sorted.sort_by(f32::total_cmp);
    let cut = (sorted.len() as f64 * input_tail) as usize;
    let graph = m.graph.as_ref().expect("the model has a graph");
    ranges.insert(graph.input[0].name.clone(), (sorted[cut], sorted[sorted.len() - 1 - cut]));
    ranges
}

/// Asymmetric int8, on a range that must contain 0.
fn qparams((lo, hi): (f32, f32)) -> (f32, i8) {
    let (lo, hi) = (lo.min(0.0), hi.max(0.0));
    let scale = ((hi - lo) / INT8_STEPS).max(SMALLEST_SCALE);
    (scale, (INT8_LOW - lo / scale).round().clamp(INT8_LOW, INT8_HIGH) as i8)
}

fn quantize(dir: &Path, source: &str, target: &str, input_tail: f64) {
    let bytes = std::fs::read(dir.join(source)).expect("the source graph reads");
    let mut m = ModelProto::decode(&bytes[..]).expect("the source graph decodes");
    let folded = fold_batch_norm(&mut m);
    let folded_path = dir.join(format!("{target}.float-folded.onnx"));
    std::fs::write(&folded_path, m.encode_to_vec()).expect("the folded graph is written");
    println!("folded {folded} BatchNormalization nodes");
    let ranges = calibrate(dir, &folded_path, input_tail);
    let g0 = m.graph.as_ref().expect("the model has a graph").clone();
    let input = g0.input[0].name.clone();
    let output = g0.output[0].name.clone();
    // Quantised: the input, Relu and Add outputs and the final logits. Conv and Relu stay one unit,
    // because ONNX Runtime's ReluQuantFusion drops the Relu when Q's zero point equals qmin.
    let mut activations: Vec<String> = vec![input.clone()];
    activations.extend(g0.node.iter().filter(|n| matches!(n.op_type.as_str(), "Relu" | "Add")).map(|n| n.output[0].clone()));
    activations.push(output.clone());
    let qp: HashMap<String, (f32, i8)> = activations.iter()
        .map(|a| (a.clone(), qparams(*ranges.get(a).unwrap_or_else(|| panic!("no range for {a}")))))
        .collect();
    let mut inits: Vec<TensorProto> = vec![];
    let mut nodes: Vec<NodeProto> = vec![];
    let dq = |t: &str| format!("{t}_dq");
    let qdq = |t: &str, nodes: &mut Vec<NodeProto>, inits: &mut Vec<TensorProto>, source: String| {
        let (scale, zero) = qp[t];
        inits.push(tf(&format!("{t}_s"), &[], &[scale]));
        inits.push(ti8(&format!("{t}_z"), &[], &[zero]));
        nodes.push(node("QuantizeLinear", format!("{t}_Q"), vec![source, format!("{t}_s"), format!("{t}_z")], vec![format!("{t}_q")], None));
        nodes.push(node("DequantizeLinear", format!("{t}_DQ"), vec![format!("{t}_q"), format!("{t}_s"), format!("{t}_z")], vec![dq(t)], None));
    };
    qdq(&input, &mut nodes, &mut inits, input.clone());
    let map = |n: &str| if qp.contains_key(n) { dq(n) } else { n.to_string() };
    let renamed_output = format!("{output}_raw");
    for n in &g0.node {
        let mut rewritten = n.clone();
        if n.op_type == "Conv" {
            let input_scale = qp[&n.input[0]].0;
            let (weight_dims, weights) = tget(&m, &n.input[1]);
            let (bias_dims, biases) = tget(&m, &n.input[2]);
            let channels = weight_dims[0] as usize;
            let per = weights.len() / channels;
            let (mut weight_scale, mut weight_q, mut bias_q, mut bias_scale) = (vec![0f32; channels], vec![0i8; weights.len()], vec![0i32; channels], vec![0f32; channels]);
            for c in 0..channels {
                let row = &weights[c * per..(c + 1) * per];
                let largest = row.iter().fold(0f32, |a, v| a.max(v.abs())).max(SMALLEST_SCALE);
                weight_scale[c] = largest / INT8_HIGH;
                for (o, v) in weight_q[c * per..(c + 1) * per].iter_mut().zip(row) {
                    *o = (v / weight_scale[c]).round().clamp(-INT8_HIGH, INT8_HIGH) as i8;
                }
                bias_scale[c] = input_scale * weight_scale[c];
                bias_q[c] = (f64::from(biases[c]) / f64::from(bias_scale[c])).round() as i32;
            }
            let (wn, bn) = (n.input[1].clone(), n.input[2].clone());
            let per_channel = [channels as i64];
            inits.push(ti8(&format!("{wn}_q"), &weight_dims, &weight_q));
            inits.push(tf(&format!("{wn}_s"), &per_channel, &weight_scale));
            inits.push(ti8(&format!("{wn}_z"), &per_channel, &vec![0i8; channels]));
            inits.push(ti32(&format!("{bn}_q"), &bias_dims, &bias_q));
            inits.push(tf(&format!("{bn}_s"), &per_channel, &bias_scale));
            inits.push(ti32(&format!("{bn}_z"), &per_channel, &vec![0i32; channels]));
            nodes.push(node("DequantizeLinear", format!("{wn}_DQ"), vec![format!("{wn}_q"), format!("{wn}_s"), format!("{wn}_z")], vec![format!("{wn}_w")], Some(0)));
            nodes.push(node("DequantizeLinear", format!("{bn}_DQ"), vec![format!("{bn}_q"), format!("{bn}_s"), format!("{bn}_z")], vec![format!("{bn}_b")], Some(0)));
            rewritten.input = vec![map(&n.input[0]), format!("{wn}_w"), format!("{bn}_b")];
        } else {
            rewritten.input = n.input.iter().map(|i| map(i)).collect();
        }
        if rewritten.output[0] == output {
            rewritten.output[0].clone_from(&renamed_output);
        }
        let t = n.output[0].clone();
        nodes.push(rewritten);
        if qp.contains_key(&t) {
            let from = if t == output { renamed_output.clone() } else { t.clone() };
            qdq(&t, &mut nodes, &mut inits, from);
        }
    }
    // The last DequantizeLinear must carry the declared output's name.
    if let Some(last) = nodes.iter_mut().rev().find(|n| n.output.first() == Some(&dq(&output))) {
        last.output[0] = output;
    }
    let g = m.graph.as_mut().expect("the model has a graph");
    g.node = nodes;
    g.initializer = inits;
    std::fs::write(dir.join(target), m.encode_to_vec()).expect("the int8 graph is written");
    println!("wrote {target}");
}

/// Runs a model over `eval{i}.bin`, one gesture at a time, writing `{prefix}{i}.bin`.
fn run(dir: &Path, model: &str, prefix: &str) {
    let mut s = session(&dir.join(model)).expect("the model loads");
    let mut index = 0;
    while dir.join(format!("eval{index}.bin")).exists() {
        let mut out: Vec<u8> = vec![];
        for one in floats(&dir.join(format!("eval{index}.bin"))).as_chunks::<INPUT_VALUES>().0 {
            let a = ArrayD::from_shape_vec(SHAPE.to_vec(), one.to_vec()).expect("one gesture");
            let view = TensorRef::from_array_view(a.view()).expect("a view");
            let result = s.run(ort::inputs![view]).expect("the model runs");
            for v in result[0].try_extract_tensor::<f32>().expect("a float tensor").1 {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write(dir.join(format!("{prefix}{index}.bin")), out).expect("the outputs are written");
        index += 1;
    }
    println!("ran {model} on {index} sets");
}

/// The C API's route to `.ort`, the one ONNX Runtime's Python converter drives.
fn convert(dir: &Path, source: &str, target: &str) -> ort::Result<()> {
    // Level 2 fuses QDQ into QLinearConv and QLinearAdd. Level 3 adds layout rewrites keyed to
    // this host's hardware, which a file for the phone must not carry.
    let mut builder = Session::builder(env())?
        .with_auto_device(AutoDevicePolicy::PreferCPU)?
        .with_intra_threads(1)?
        .with_optimization_level(GraphOptimizationLevel::Level2)?
        .with_optimized_model_path(dir.join(target).to_string_lossy().as_ref())?
        .with_config_entry("session.save_model_format", "ORT")?;
    builder.commit_from_file(dir.join(source))?;
    println!("wrote {target}");
    Ok(())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let tail = a.iter().position(|x| x == "--input-tail").and_then(|i| a.get(i + 1)).map_or(0.0, |v| v.parse().expect("--input-tail is a share"));
    match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["quant", dir, source, target, ..] => quantize(&PathBuf::from(dir), source, target, tail),
        ["run", dir, model, prefix] => run(&PathBuf::from(dir), model, prefix),
        ["convert", dir, source, target] => convert(&PathBuf::from(dir), source, target).expect("the conversion runs"),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
