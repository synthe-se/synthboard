use crate::data::{Batch, KEYS};
use burn::module::{Module, ModuleMapper, ModuleVisitor, Param};
use burn::nn::conv::{Conv1d, Conv1dConfig};
use burn::nn::{BatchNorm, BatchNormConfig, Linear, LinearConfig, PaddingConfig1d};
use burn::prelude::*;
use burn::tensor::TensorData;
use burn::tensor::activation::{log_softmax, relu};
use synthswipe::{ABSENT, FEATURES, FRAMES, KEY_DIMENSION};

/// Reaches across a whole gesture of `FRAMES` frames with few parameters.
const DILATIONS: [usize; 7] = [1, 2, 4, 8, 1, 2, 4];
const KERNEL: usize = 5;

#[derive(Module, Debug)]
pub struct Block {
    dw: Conv1d,
    norm: BatchNorm,
    pw: Conv1d,
}

impl Block {
    fn forward(&self, h: Tensor<3>) -> Tensor<3> {
        let y = relu(self.norm.forward(self.dw.forward(h.clone())));
        h + relu(self.pw.forward(y))
    }
}

/// The shared body, [B, F, T] to [B, C, T]. The training graph and the exported one must match.
fn body(input: &Conv1d, blocks: &[Block], x: Tensor<3>) -> Tensor<3> {
    blocks.iter().fold(relu(input.forward(x)), |h, block| block.forward(h))
}

/// A dilated depthwise TCN. Its head scores each key from the key's position, so that the
/// layout stays data.
#[derive(Module, Debug)]
pub struct Encoder {
    input: Conv1d,
    blocks: Vec<Block>,
    query: Conv1d,
    blank: Conv1d,
    key_in: Linear,
    key_out: Linear,
}

impl Encoder {
    pub fn new(c: usize, d: &Device) -> Self {
        let blocks = DILATIONS.iter().map(|&dl| {
            let p = (KERNEL - 1) * dl / 2;
            Block {
                dw: Conv1dConfig::new(c, c, KERNEL).with_dilation(dl).with_groups(c)
                    .with_padding(PaddingConfig1d::Explicit(p, p)).init(d),
                norm: BatchNormConfig::new(c).init(d),
                pw: Conv1dConfig::new(c, c, 1).init(d),
            }
        }).collect();
        Self {
            input: Conv1dConfig::new(FEATURES, c, 1).init(d),
            blocks,
            query: Conv1dConfig::new(c, KEY_DIMENSION, 1).init(d),
            blank: Conv1dConfig::new(c, 1, 1).init(d),
            key_in: LinearConfig::new(2, KEY_DIMENSION).init(d),
            key_out: LinearConfig::new(KEY_DIMENSION, KEY_DIMENSION).init(d),
        }
    }

    /// Logits [B, CLASSES, T].
    fn key_scores(&self, query: Tensor<3>, blank: Tensor<3>, keys: Tensor<3>, mask: Tensor<2>) -> Tensor<3> {
        let k = self.key_out.forward(relu(self.key_in.forward(keys)));
        let absent: Tensor<3> = (mask.ones_like() - mask).unsqueeze_dim(2);
        Tensor::cat(vec![blank, k.matmul(query) + absent * ABSENT], 1)
    }

    /// Logits [B, CLASSES, T].
    pub fn logits(&self, x: Tensor<3>, keys: Tensor<3>, mask: Tensor<2>) -> Tensor<3> {
        let h = body(&self.input, &self.blocks, x);
        self.key_scores(self.query.forward(h.clone()), self.blank.forward(h), keys, mask)
    }

    /// The same logits, from what the exported graph returns: [B, 1 + D, T].
    pub fn scores(&self, out: Tensor<3>, keys: Tensor<3>, mask: Tensor<2>) -> Tensor<3> {
        let [b, _, t] = out.dims();
        let blank = out.clone().slice([0..b, KEY_DIMENSION..KEY_DIMENSION + 1, 0..t]);
        let query = out.slice([0..b, 0..KEY_DIMENSION, 0..t]);
        self.key_scores(query, blank, keys, mask)
    }

    /// Merges query and blank into one convolution, so that the graph is only Conv,
    /// BatchNormalization, Relu and Add.
    pub fn deploy(&self, d: &Device) -> Deploy {
        let (q, b) = (&self.query, &self.blank);
        let c = q.weight.val().dims()[1];
        let mut head = Conv1dConfig::new(c, KEY_DIMENSION + 1, 1).init(d);
        head.weight = Param::from_tensor(Tensor::cat(vec![q.weight.val(), b.weight.val()], 0));
        if let (Some(query), Some(blank)) = (&q.bias, &b.bias) {
            head.bias = Some(Param::from_tensor(Tensor::cat(vec![query.val(), blank.val()], 0)));
        }
        Deploy { input: self.input.clone(), blocks: self.blocks.clone(), head }
    }

    /// The key head as the runtime reads it: weights by row, then bias, for each layer.
    pub fn head_values(&self) -> Vec<f32> {
        let mut all = vec![];
        for layer in [&self.key_in, &self.key_out] {
            all.extend(values(layer.weight.val()));
            all.extend(layer.bias.iter().flat_map(|bias| values(bias.val())));
        }
        all
    }

    /// Log-probabilities [T, B, CLASSES], the shape CTC takes.
    pub fn log_probs(&self, x: Tensor<3>, keys: Tensor<3>, mask: Tensor<2>) -> Tensor<3> {
        log_softmax(self.logits(x, keys, mask).permute([2, 0, 1]), 2)
    }
}

/// A tensor's values, which are `f32` whatever the backend's float type.
pub fn values<const D: usize>(t: Tensor<D>) -> Vec<f32> {
    t.into_data().convert::<f32>().try_to_vec::<f32>().expect("converted to f32 above")
}

pub struct Inputs {
    pub x: Tensor<3>,
    pub keys: Tensor<3>,
    pub mask: Tensor<2>,
}

pub fn inputs(b: &Batch, d: &Device) -> Inputs {
    Inputs {
        x: Tensor::<3>::from_data(TensorData::new(b.x.clone(), [b.n, FEATURES, FRAMES]), d),
        keys: Tensor::<3>::from_data(TensorData::new(b.keys.clone(), [b.n, KEYS, 2]), d),
        mask: Tensor::<2>::from_data(TensorData::new(b.mask.clone(), [b.n, KEYS]), d),
    }
}

// Burn has no EMA: walk the parameters, flattened since visitors are per rank.
struct Grab(Vec<Tensor<1>>);
impl ModuleVisitor for Grab {
    fn visit_float<const D: usize>(&mut self, p: &Param<Tensor<D>>) {
        self.0.push(p.val().flatten(0, D - 1).detach());
    }
}
struct Blend {
    live: std::vec::IntoIter<Tensor<1>>,
    decay: f32,
}
impl ModuleMapper for Blend {
    fn map_float<const D: usize>(&mut self, p: Param<Tensor<D>>) -> Param<Tensor<D>> {
        let (id, t, mapper) = p.consume();
        let shape = t.shape();
        let live = self.live.next().expect("both models have the same parameters").reshape(shape);
        Param::from_mapped_value(id, (t * self.decay + live * (1.0 - self.decay)).detach(), mapper)
    }
}
pub fn ema(shadow: Encoder, live: &Encoder, decay: f32) -> Encoder {
    let mut grabbed = Grab(vec![]);
    live.visit(&mut grabbed);
    shadow.map(&mut Blend { live: grabbed.0.into_iter(), decay })
}

#[derive(Module, Debug)]
pub struct Deploy {
    input: Conv1d,
    blocks: Vec<Block>,
    head: Conv1d,
}
impl Deploy {
    pub fn forward(&self, x: Tensor<3>) -> Tensor<3> {
        self.head.forward(body(&self.input, &self.blocks, x))
    }
}
