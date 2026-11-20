//! ZIM → pack, delegated whole to `zim2wax` (Track B B1).
//!
//! WSS adds nothing to the conversion itself on purpose: the box converts a
//! ZIM from a USB drive with the same code (decided 2026-11-15), so a ZIM
//! converted in WSS and on a box give the same pack, byte for byte, given the
//! same identity, timestamps and level. WSS's part is the recipe: it maps the
//! recipe onto `zim2wax`'s options and builds at level 19 (ADR-0003).
//!
//! A ZIM states its own name, version (from `Date`), languages and start
//! page, so a recipe that sets any of those for a ZIM is refused rather than
//! silently ignored.

use crate::recipe::Recipe;
use anyhow::{bail, Result};
use std::path::Path;

pub fn options(recipe: &Recipe) -> Result<zim2wax::ConvertOptions> {
    recipe.validate()?;
    let p = &recipe.pack;
    for (key, set) in [
        ("name", p.name.is_some()),
        ("version", p.version.is_some()),
        ("languages", p.languages.is_some()),
        ("entry_point", p.entry_point.is_some()),
        ("guest_accessible", p.guest_accessible.is_some()),
    ] {
        if set {
            bail!(
                "{key} cannot be set for a ZIM: the ZIM states it (Track B §20), and zim2wax \
                 derives it so a box converting the same ZIM gets the same pack. Remove it from \
                 the recipe."
            );
        }
    }
    let o = zim2wax::ConvertOptions {
        category: p.category.clone().unwrap_or_default(),
        min_ram_bytes: p.min_ram_bytes.unwrap_or_default(),
        min_storage_bytes: p.min_storage_bytes.unwrap_or_default(),
        arch: p.arch.clone().unwrap_or_default(),
        gpu: p.gpu.clone(),
        license_if_absent: p.license.clone(),
        attribution_if_absent: p.attribution.clone(),
        created_at: recipe.created_at(),
        archive_uuid: recipe.archive_uuid()?,
        sign_key: recipe.sign_key(),
        carry_dictionary_from: recipe.build.carry_dictionary_from.clone(),
        search_index: recipe.build.search_index,
        zstd_level: Some(recipe.build.zstd_level),
    };
    o.validate()?;
    Ok(o)
}

pub fn convert(zim: &Path, output: &Path, recipe: &Recipe) -> Result<zim2wax::ConvertReport> {
    zim2wax::convert(zim, output, &options(recipe)?)
}
