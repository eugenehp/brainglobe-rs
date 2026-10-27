//! Structure meshes: the Draco files of an annotation set's
//! `annotations.precomputed/mesh` (`Structure._read_mesh`).
//!
//! Each file is one Draco mesh in neuroglancer's coordinates (x, y, z, in
//! nanometres). Reading it decodes it (with `draco-core`, which gives the
//! points and faces DracoPy gives), scales it to microns and reorders the axes
//! to the atlas's (z, y, x), as Python does, into what meshio would hold.

use std::path::Path;

use draco_core::{DecoderBuffer, GeometryAttributeType, MeshDecoder, PointIndex};

use crate::error::{Error, Result};

/// A triangle mesh (`meshio.Mesh` with one `triangle` cell block).
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    /// points, in microns, (z, y, x): `float32`, as `points / 1000.0` leaves
    /// DracoPy's float32 points in numpy (the division is done in float32)
    pub points: Vec<[f32; 3]>,
    /// triangles, as point indices (the decoded faces with their corners
    /// reversed)
    pub cells: Vec<[u32; 3]>,
}

/// A decoded Draco mesh, as DracoPy returns it: float32 points (in file
/// order) and faces.
#[derive(Debug, Clone, PartialEq)]
pub struct DracoMesh {
    pub points: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
}

/// Decode a Draco mesh (`DracoPy.decode`).
pub fn draco_decode(bytes: &[u8]) -> Result<DracoMesh> {
    let mut buf = DecoderBuffer::new(bytes);
    let mut mesh = draco_core::Mesh::new();
    MeshDecoder::new()
        .decode(&mut buf, &mut mesh)
        .map_err(|e| Error::value(format!("Failed to decode buffer: {e:?}")))?;
    let pos = mesh
        .named_attribute(GeometryAttributeType::Position)
        .ok_or_else(|| Error::value("Draco mesh has no position attribute"))?;
    let n = pos.num_components() as usize;
    if n != 3 || pos.data_type() != draco_core::DataType::Float32 {
        return Err(Error::value(format!(
            "unsupported position attribute: {n} x {:?}",
            pos.data_type()
        )));
    }
    let stride = pos.byte_stride() as usize;
    let data = pos.buffer().data();
    let mut points = Vec::with_capacity(mesh.num_points());
    for i in 0..mesh.num_points() {
        let v = pos.mapped_index(PointIndex(i as u32)).0 as usize;
        let o = v * stride;
        let f = |k: usize| f32::from_le_bytes(data[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
        points.push([f(0), f(1), f(2)]);
    }
    let faces = mesh
        .faces()
        .iter()
        .map(|f| [f[0].0, f[1].0, f[2].0])
        .collect();
    Ok(DracoMesh { points, faces })
}

impl Mesh {
    /// Read one precomputed mesh file.
    pub fn read_precomputed(path: impl AsRef<Path>) -> Result<Mesh> {
        let bytes = std::fs::read(path)?;
        Mesh::from_draco(&bytes)
    }

    /// Decode a precomputed mesh file's bytes.
    pub fn from_draco(bytes: &[u8]) -> Result<Mesh> {
        let m = draco_decode(bytes)?;
        Ok(Mesh {
            // scale from nm to um, reorient from XYZ to ZYX
            points: m
                .points
                .iter()
                .map(|p| [p[2] / 1000.0f32, p[1] / 1000.0f32, p[0] / 1000.0f32])
                .collect(),
            cells: m.faces.iter().map(|f| [f[2], f[1], f[0]]).collect(),
        })
    }
}
