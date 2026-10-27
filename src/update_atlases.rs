//! Installing and updating atlases (`brainglobe_atlasapi.update_atlases`).

use crate::bg_atlas::{AtlasOptions, BrainGlobeAtlas, version_str_from_tuple};
use crate::error::{Error, Result};
use crate::list_atlases::get_downloaded_atlases;
use crate::rich::rprint;

/// Update an atlas to the latest version online (`update_atlas`); `force`
/// downloads it again even if it is current.
pub fn update_atlas(atlas_name: &str, force: bool) -> Result<()> {
    let mut atlas = BrainGlobeAtlas::new(
        atlas_name,
        AtlasOptions {
            check_latest: Some(false),
            ..Default::default()
        },
    )?;
    let is_latest = atlas.check_latest_version(false)?;
    if force && is_latest == Some(true) {
        // delete the atlas folder to force the update
        let name = atlas.local_full_name()?.unwrap_or_default();
        let atlas_folder = atlas
            .brainglobe_dir
            .join(name)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&atlas_folder);
        if atlas_folder.exists() {
            return Err(Error::value(
                "Something went wrong while trying to delete the old version of the atlas, aborting.",
            ));
        }
    } else if is_latest == Some(true) {
        let version = version_str_from_tuple(&atlas.local_version()?);
        rprint(&format!(
            "[b][magenta2]brainglobe_atlasapi: {} is already updated (version: {version})[/b]",
            atlas.atlas_name
        ));
        return Ok(());
    }
    rprint(&format!(
        "[b][magenta2]brainglobe_atlasapi: updating {}[/magenta2][/b]",
        atlas.atlas_name
    ));
    atlas.download()?;
    let remote = atlas.remote_version()?.unwrap_or_default();
    rprint(&format!(
        "[b][magenta2]brainglobe_atlasapi: {} updated to version: {}[/magenta2][/b]",
        atlas.atlas_name,
        version_str_from_tuple(&remote)
    ));
    Ok(())
}

/// Install an atlas from the latest version online (`install_atlas`).
pub fn install_atlas(atlas_name: &str) -> Result<()> {
    if get_downloaded_atlases()?.iter().any(|a| a == atlas_name) {
        rprint(&format!(
            "[b][magenta2]brainglobe_atlasapi: installing {atlas_name}: atlas already installed![/magenta2][/b]"
        ));
        return Ok(());
    }
    BrainGlobeAtlas::new(atlas_name, AtlasOptions::default())?;
    Ok(())
}
