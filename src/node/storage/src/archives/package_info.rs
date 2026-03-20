/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    archives::{package::Package, package_entry_meta_db::PackageEntryInfo, package_id::PackageId},
    StorageAlloc,
};
use adnl::{
    common::{CountedObject, Counter},
    declare_counted,
};
#[cfg(feature = "telemetry")]
use std::sync::atomic::Ordering;
use std::sync::Arc;
use ton_block::Result;

declare_counted!(
    pub(crate) struct PackageInfo {
        entry: PackageEntryInfo,
        index: u32,
        package_id: PackageId,
        package: Package,
        version: u32,
    }
);

impl PackageInfo {
    pub fn with_data(
        entry: PackageEntryInfo,
        index: u32,
        package_id: PackageId,
        package: Package,
        version: u32,
        #[cfg(feature = "telemetry")] telemetry: &Arc<StorageTelemetry>,
        allocated: &Arc<StorageAlloc>,
    ) -> Self {
        #[cfg(feature = "telemetry")]
        telemetry.packages.update(allocated.packages.load(Ordering::Relaxed));
        Self {
            entry,
            index,
            package_id,
            package,
            version,
            counter: allocated.packages.clone().into(),
        }
    }

    pub async fn destroy(&mut self) -> Result<()> {
        self.package.remove().await
    }

    pub const fn entry(&self) -> &PackageEntryInfo {
        &self.entry
    }

    pub const fn index(&self) -> u32 {
        self.index
    }

    pub const fn package(&self) -> &Package {
        &self.package
    }

    pub fn package_mut(&mut self) -> &mut Package {
        &mut self.package
    }

    pub const fn version(&self) -> u32 {
        self.version
    }
}
