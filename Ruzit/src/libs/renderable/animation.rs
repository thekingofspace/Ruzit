use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::mesh::{Vertex3D, deform_blob, recompute_normals};

static NEXT_TRACK_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone)]
pub enum KeyframeAction {
    Cframe(CFrame),

    Deform {
        center: [f32; 3],
        envelope: f32,
        displacement: [f32; 3],
    },
}

#[derive(Clone)]
pub struct Keyframe {
    pub time: f32,
    pub action: KeyframeAction,
}

#[derive(Clone, Default)]
pub struct TrackBaseline {
    pub cframe: Option<CFrame>,
    pub vertices: Option<Arc<Vec<Vertex3D>>>,
}

pub struct AnimationTrack {
    #[allow(dead_code)]
    pub id: u64,
    pub name: String,
    pub fade_time: f32,
    pub looped: bool,
    pub speed: f32,
    pub time: f32,
    pub state: TrackState,
    pub keyframes: Vec<Keyframe>,
    pub duration: f32,
    pub baseline: TrackBaseline,
}

impl AnimationTrack {
    pub fn new(name: String) -> Self {
        Self {
            id: NEXT_TRACK_ID.fetch_add(1, Ordering::Relaxed),
            name,
            fade_time: 0.0,
            looped: false,
            speed: 1.0,
            time: 0.0,
            state: TrackState::Stopped,
            keyframes: Vec::new(),
            duration: 0.0,
            baseline: TrackBaseline::default(),
        }
    }

    pub fn add_keyframe(&mut self, kf: Keyframe) {
        let pos = self
            .keyframes
            .binary_search_by(|k| {
                k.time
                    .partial_cmp(&kf.time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or_else(|p| p);
        self.keyframes.insert(pos, kf);
        self.duration = self
            .keyframes
            .iter()
            .map(|k| k.time)
            .fold(0.0_f32, f32::max);
    }
}

pub type TrackRef = Arc<Mutex<AnimationTrack>>;

fn lerp_cframe(a: CFrame, b: CFrame, t: f32) -> CFrame {
    let pa = a.position;
    let pb = b.position;
    let ra = a.rotation;
    let rb = b.rotation;
    CFrame::new(
        Vector::new(
            pa.x + (pb.x - pa.x) * t,
            pa.y + (pb.y - pa.y) * t,
            pa.z + (pb.z - pa.z) * t,
        ),
        Vector::new(
            ra.x + (rb.x - ra.x) * t,
            ra.y + (rb.y - ra.y) * t,
            ra.z + (rb.z - ra.z) * t,
        ),
    )
}

pub struct TrackEval {
    pub cframe_override: Option<CFrame>,
    pub vertices_override: Option<Arc<Vec<Vertex3D>>>,
    pub indices_for_normals: Option<Arc<Vec<u32>>>,
    pub finished: bool,
}

pub fn tick_track(
    track: &mut AnimationTrack,
    dt: f32,
    base_cframe: CFrame,
    base_vertices: Arc<Vec<Vertex3D>>,
    base_indices: Arc<Vec<u32>>,
) -> TrackEval {
    let mut eval = TrackEval {
        cframe_override: None,
        vertices_override: None,
        indices_for_normals: None,
        finished: false,
    };
    if track.state != TrackState::Playing {
        return eval;
    }
    if track.keyframes.is_empty() {
        track.state = TrackState::Stopped;
        eval.finished = true;
        return eval;
    }

    track.time += dt * track.speed;
    if track.time > track.duration {
        if track.looped && track.duration > 0.0 {
            track.time = track.time % track.duration;
        } else {
            track.time = track.duration;
            track.state = TrackState::Stopped;
            eval.finished = true;
        }
    }
    let t = track.time;

    let cf_kfs: Vec<&Keyframe> = track
        .keyframes
        .iter()
        .filter(|k| matches!(k.action, KeyframeAction::Cframe(_)))
        .collect();
    if !cf_kfs.is_empty() {
        let target = if t <= cf_kfs[0].time {
            match cf_kfs[0].action {
                KeyframeAction::Cframe(cf) => {
                    let blend = if cf_kfs[0].time > 0.0 {
                        (t / cf_kfs[0].time).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    lerp_cframe(base_cframe, cf, blend)
                }
                _ => base_cframe,
            }
        } else {
            let mut found = None;
            for w in cf_kfs.windows(2) {
                if t >= w[0].time && t <= w[1].time {
                    let span = (w[1].time - w[0].time).max(1e-6);
                    let blend = ((t - w[0].time) / span).clamp(0.0, 1.0);
                    let (KeyframeAction::Cframe(a), KeyframeAction::Cframe(b)) =
                        (w[0].action.clone(), w[1].action.clone())
                    else {
                        continue;
                    };
                    found = Some(lerp_cframe(a, b, blend));
                    break;
                }
            }
            found.unwrap_or_else(|| match cf_kfs.last().unwrap().action {
                KeyframeAction::Cframe(cf) => cf,
                _ => base_cframe,
            })
        };
        eval.cframe_override = Some(target);
    }

    let mut deform_kfs: Vec<&Keyframe> = track
        .keyframes
        .iter()
        .filter(|k| matches!(k.action, KeyframeAction::Deform { .. }) && k.time <= t)
        .collect();
    if !deform_kfs.is_empty() {
        deform_kfs.sort_by(|a, b| {
            a.time
                .partial_cmp(&b.time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut working: Vec<Vertex3D> = (*base_vertices).clone();
        for kf in deform_kfs {
            if let KeyframeAction::Deform {
                center,
                envelope,
                displacement,
            } = kf.action
            {
                deform_blob(&mut working, center, envelope, displacement);
            }
        }
        recompute_normals(&mut working, &base_indices);
        eval.vertices_override = Some(Arc::new(working));
        eval.indices_for_normals = Some(base_indices);
    }

    eval
}
