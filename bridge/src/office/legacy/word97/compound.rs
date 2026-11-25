// SPDX-License-Identifier: GPL-3.0-or-later
//! cfb builds its FAT, MiniFAT and directory in memory. Verify their actual
//! chain lengths before handing it a document; old CFB headers can omit counts.
use super::{u16_at, u32_at};
const METADATA_BUDGET: u64 = 64 * 1024 * 1024;
use anyhow::{anyhow, ensure, Result};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const FREE: u32 = 0xffff_ffff;
const END: u32 = 0xffff_fffe;

pub(super) fn preflight(file: &mut File) -> Result<()> {
    let len = file.metadata()?.len();
    file.rewind()?;
    let mut header = [0; 512];
    file.read_exact(&mut header)
        .map_err(|_| anyhow!("corrupt_document: incomplete compound header"))?;
    let shift = u16_at(&header, 30)?;
    ensure!(
        matches!(shift, 9 | 12),
        "corrupt_document: invalid compound sector size"
    );
    let sector_len = 1u64 << shift;
    let slots = sector_len as usize / 4;
    let sectors = len.div_ceil(sector_len).saturating_sub(1);
    let sector_offset = |id: u32| -> Result<u64> {
        ensure!(
            u64::from(id) < sectors,
            "corrupt_document: compound sector outside file"
        );
        Ok((u64::from(id) + 1) * sector_len)
    };
    let budget = |used: u64| {
        ensure!(
            used <= METADATA_BUDGET,
            "resource_limit: compound file metadata"
        );
        Ok::<(), anyhow::Error>(())
    };
    // cfb pads the FAT to the physical sector count even when the header claims fewer sectors.
    budget(sectors * 4)?;
    let mut fat: Vec<u32> = header[76..]
        .chunks_exact(4)
        .map(|data| u32::from_le_bytes(data.try_into().unwrap()))
        .collect();
    let mut difat = u32_at(&header, 68)?;
    let mut visited = HashSet::new();
    let mut sector = vec![0; sector_len as usize];
    while difat != END && difat != FREE {
        crate::search::checkpoint()?;
        ensure!(
            visited.insert(difat),
            "corrupt_document: circular compound allocation table"
        );
        file.seek(SeekFrom::Start(sector_offset(difat)?))?;
        file.read_exact(&mut sector)?;
        for entry in sector[..sector.len() - 4].chunks_exact(4) {
            fat.push(u32::from_le_bytes(entry.try_into().unwrap()));
        }
        difat = u32_at(&sector, sector.len() - 4)?;
        // One DIFAT entry references one complete FAT sector; at most the
        // last DIFAT sector can have unused padding.
        budget(fat.len().saturating_sub(slots).saturating_sub(109) as u64 * sector_len)?;
    }
    while fat.last() == Some(&FREE) {
        fat.pop();
    }
    let stated_fat = u32_at(&header, 44)? as usize;
    while fat.len() > 109 && fat.len() > stated_fat && fat.last() == Some(&0) {
        fat.pop();
    }
    let mut used = (fat.len() as u64 * sector_len).max(sectors * 4);
    budget(used)?;
    // A permissive reader may follow more directory/MiniFAT sectors than the
    // corresponding header declares. Read their FAT links without loading the
    // sector contents or duplicating the complete allocation table.
    for (field, cost) in [(48, sector_len * 2), (60, sector_len * 2)] {
        let mut id = u32_at(&header, field)?;
        visited.clear();
        let mut cached = None;
        while id != END && id != FREE {
            crate::search::checkpoint()?;
            sector_offset(id)?;
            ensure!(
                visited.insert(id),
                "corrupt_document: circular compound metadata"
            );
            used += cost;
            budget(used)?;
            let page = id as usize / slots;
            if cached != Some(page) {
                let fat_id = *fat.get(page).ok_or_else(|| {
                    anyhow!("corrupt_document: incomplete compound allocation table")
                })?;
                file.seek(SeekFrom::Start(sector_offset(fat_id)?))?;
                file.read_exact(&mut sector)?;
                cached = Some(page);
            }
            id = u32_at(&sector, id as usize % slots * 4)?;
        }
    }
    file.rewind()?;
    Ok(())
}
