/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use ton_block::{error, fail, Result, ShardIdent};

pub struct PackageFile {
    pub path: PathBuf,
    pub archive_id: u32,
    pub shard: ShardIdent,
}

pub struct PackageGroup {
    pub archive_id: u32,
    pub mc_package: PackageFile,
    pub shard_packages: Vec<PackageFile>,
}

/// Parse a .pack filename into (archive_id, shard).
fn parse_pack_filename(filename: &str) -> Result<Option<(u32, ShardIdent)>> {
    if !filename.ends_with(".pack") {
        return Ok(None);
    }
    let stem = &filename[..filename.len() - 5];

    if stem.starts_with("key.") {
        return Ok(None);
    }

    if !stem.starts_with("archive.") {
        return Ok(None);
    }
    let rest = &stem[8..];

    if let Some(dot_pos) = rest.find('.') {
        // archive.NNNNN.WC:HHHHHHHHHHHHHHHH - shards
        let id_str = &rest[..dot_pos];
        let shard_str = &rest[dot_pos + 1..];

        let archive_id: u32 =
            id_str.parse().map_err(|_| error!("Invalid archive id in filename: {}", filename))?;

        let shard = ShardIdent::from_str(shard_str)?;
        Ok(Some((archive_id, shard)))
    } else {
        // archive.NNNNN — masterchain
        let archive_id: u32 =
            rest.parse().map_err(|_| error!("Invalid archive id in filename: {}", filename))?;
        Ok(Some((archive_id, ShardIdent::masterchain())))
    }
}

/// Scan the source directory for .pack files, parse filenames, sort by archive_id.
pub fn scan_packages(archives_path: &Path) -> Result<Vec<PackageFile>> {
    let entries = std::fs::read_dir(archives_path)
        .map_err(|e| error!("Cannot read archives directory {}: {}", archives_path.display(), e))?;

    let mut packages = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| error!("Error reading directory entry: {}", e))?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        if let Some((archive_id, shard)) = parse_pack_filename(&filename)? {
            packages.push(PackageFile { path, archive_id, shard });
        }
    }

    // Sort by archive_id, then MC before shards
    packages.sort_by(|a, b| {
        a.archive_id.cmp(&b.archive_id).then_with(|| {
            let a_mc = a.shard.is_masterchain() as u8;
            let b_mc = b.shard.is_masterchain() as u8;
            b_mc.cmp(&a_mc) // MC first
        })
    });

    Ok(packages)
}

/// Group packages by archive_id: each group has one MC package and zero or more shard packages.
pub fn group_by_archive_id(packages: Vec<PackageFile>) -> Result<Vec<PackageGroup>> {
    let mut map: BTreeMap<u32, (Option<PackageFile>, Vec<PackageFile>)> = BTreeMap::new();

    for pkg in packages {
        let entry = map.entry(pkg.archive_id).or_insert_with(|| (None, Vec::new()));
        if pkg.shard.is_masterchain() {
            if entry.0.is_some() {
                fail!("Duplicate MC package for archive_id {}", pkg.archive_id);
            }
            entry.0 = Some(pkg);
        } else {
            entry.1.push(pkg);
        }
    }

    let mut groups = Vec::with_capacity(map.len());
    for (archive_id, (mc_package, shard_packages)) in map {
        let mc_package = mc_package
            .ok_or_else(|| error!("No MC package found for archive_id {}", archive_id))?;
        groups.push(PackageGroup { archive_id, mc_package, shard_packages });
    }

    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mc_filename() {
        let (id, shard) = parse_pack_filename("archive.00100.pack").unwrap().unwrap();
        assert_eq!(id, 100);
        assert!(shard.is_masterchain());
    }

    #[test]
    fn test_parse_shard_filename_with_wc() {
        let (id, shard) =
            parse_pack_filename("archive.00100.0:8000000000000000.pack").unwrap().unwrap();
        assert_eq!(id, 100);
        assert!(!shard.is_masterchain());
        assert_eq!(shard.workchain_id(), 0);
        assert_eq!(shard.shard_prefix_with_tag(), 0x8000000000000000);
    }

    #[test]
    fn test_parse_key_filename_skipped() {
        assert!(parse_pack_filename("key.archive.000000.pack").unwrap().is_none());
    }

    #[test]
    fn test_parse_non_pack_file() {
        assert!(parse_pack_filename("readme.txt").unwrap().is_none());
    }
}
