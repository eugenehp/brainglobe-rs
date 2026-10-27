//! Constants of the atlas format (`brainglobe_atlasapi.descriptors`): the
//! bucket, file and directory names of the v3 layout, the stacks' types and
//! the standard orientation.

#[cfg(feature = "zarr")]
use ome_zarr::array::DType;

/// Base URL for the BrainGlobe atlas S3 bucket (`{}`: the key).
pub const REMOTE_URL_S3: &str = "s3://brainglobe/atlas/{}";
/// The same bucket over https (anonymous reads).
pub const REMOTE_URL_S3_HTTP: &str = "https://brainglobe.s3.us-west-2.amazonaws.com/atlas/{}";

/// `remote_url_s3.format(key)`.
pub fn remote_url_s3(key: &str) -> String {
    REMOTE_URL_S3.replace("{}", key)
}

/// `remote_url_s3_http.format(key)`.
pub fn remote_url_s3_http(key: &str) -> String {
    REMOTE_URL_S3_HTTP.replace("{}", key)
}

/// Major version of atlases used by this release.
pub const ATLAS_MAJOR_V: u32 = 0;

/// Supported resolution units.
pub const RESOLUTION: [&str; 3] = ["nm", "um", "mm"];

// File and directory names of the (v2) atlas package:
pub const METADATA_FILENAME: &str = "metadata.json";
pub const STRUCTURES_FILENAME: &str = "structures.json";
pub const REFERENCE_FILENAME: &str = "reference.tiff";
pub const ANNOTATION_FILENAME: &str = "annotation.tiff";
pub const HEMISPHERES_FILENAME: &str = "hemispheres.tiff";
pub const MESHES_DIRNAME: &str = "meshes";

// V3 file names
pub const V3_ATLAS_ROOTDIR: &str = "atlases";
pub const V3_ANNOTATION_ROOTDIR: &str = "annotation-sets";
pub const V3_COORDINATE_SPACE_ROOTDIR: &str = "coordinate-spaces";
pub const V3_TEMPLATE_ROOTDIR: &str = "templates";
pub const V3_TERMINOLOGY_ROOTDIR: &str = "terminologies";
pub const V3_TERMINOLOGY_NAME: &str = "terminology.csv";
pub const V3_MESHES_DIRECTORY: &str = "annotations.precomputed/mesh";
pub const V3_TEMPLATE_NAME: &str = "template.ome.zarr";
pub const V3_ANNOTATION_NAME: &str = "annotations_compressed.ome.zarr";
pub const V3_ANNOTATION_MASKS_NAME: &str = "annotations.ome.zarr";
pub const V3_ANNOTATION_MAP_NAME: &str = "annotation_values";
pub const V3_HEMISPHERES_NAME: &str = "hemispheres.ome.zarr";

// Types of the atlas stacks:
#[cfg(feature = "zarr")]
pub const REFERENCE_DTYPE: DType = DType::UInt16;
#[cfg(feature = "zarr")]
pub const ANNOTATION_DTYPE: DType = DType::UInt32;
#[cfg(feature = "zarr")]
pub const HEMISPHERES_DTYPE: DType = DType::UInt8;
#[cfg(feature = "zarr")]
pub const ANNOTATION_MASKS_DTYPE: DType = DType::UInt8;

/// Standard orientation origin: Anterior, Superior, Right (brainglobe-space).
pub const ATLAS_ORIENTATION: &str = "asr";

/// Format the component stub for a given component name, version, root
/// directory and file name (`format_component_stub`).
pub fn format_component_stub(
    component_name: &str,
    component_version: &str,
    component_root_dir: &str,
    component_file_name: &str,
) -> String {
    let component_version = component_version.replace('.', "_");
    format!("{component_root_dir}/{component_name}/{component_version}/{component_file_name}")
}

/// The hemispheres stub of an annotation set (`format_hemispheres_stub`).
pub fn format_hemispheres_stub(annotation_name: &str, version: &str) -> String {
    format_component_stub(
        annotation_name,
        version,
        V3_ANNOTATION_ROOTDIR,
        V3_HEMISPHERES_NAME,
    )
}

/// The meshes stub of an annotation set (`format_meshes_stub`).
pub fn format_meshes_stub(annotation_name: &str, version: &str) -> String {
    format_component_stub(
        annotation_name,
        version,
        V3_ANNOTATION_ROOTDIR,
        V3_MESHES_DIRECTORY,
    )
}
