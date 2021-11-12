use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;

use crate::catalog::RootCatalog;
use crate::storage::secondary::version_manager::{EpochOp, VersionManager};
use crate::storage::secondary::{manifest::*, DeleteVector};

use super::{DiskRowset, Manifest, SecondaryStorage, StorageOptions, StorageResult};
use moka::future::Cache;
use parking_lot::RwLock;
use tokio::fs;
use tokio::sync::Mutex;

impl SecondaryStorage {
    pub(super) async fn bootstrap(options: StorageOptions) -> StorageResult<Self> {
        let catalog = RootCatalog::new();
        let tables = HashMap::new();

        // create folder if not exist
        if fs::metadata(&options.path).await.is_err() {
            info!("create db directory at {:?}", options.path);
            fs::create_dir(&options.path).await.unwrap();
        }

        // create DV folder if not exist
        let dv_directory = options.path.join("dv");
        if fs::metadata(&dv_directory).await.is_err() {
            fs::create_dir(&dv_directory).await.unwrap();
        }

        let mut manifest = Manifest::open(options.path.join("manifest.json")).await?;

        let manifest_ops = manifest.replay().await?;

        let engine = Self {
            catalog: Arc::new(catalog),
            tables: RwLock::new(tables),
            block_cache: Cache::new(options.cache_size),
            options: Arc::new(options),
            next_id: Arc::new((AtomicU32::new(0), AtomicU64::new(0))),
            version: Arc::new(VersionManager::new(manifest)),
            compactor_handler: Mutex::new((None, None)),
        };

        info!("applying {} manifest entries", manifest_ops.len());

        let mut rowsets_to_open = HashMap::new();
        let mut dvs_to_open = HashMap::new();

        for op in manifest_ops {
            match op {
                ManifestOperation::CreateTable(entry) => {
                    engine.apply_create_table(&entry)?;
                }
                ManifestOperation::DropTable(entry) => {
                    engine.apply_drop_table(&entry)?;
                }
                ManifestOperation::AddRowSet(entry) => {
                    engine
                        .next_id
                        .0
                        .fetch_max(entry.rowset_id + 1, std::sync::atomic::Ordering::SeqCst);

                    rowsets_to_open.insert((entry.table_id.table_id, entry.rowset_id), entry);
                }
                ManifestOperation::DeleteRowSet(entry) => {
                    rowsets_to_open.remove(&(entry.table_id.table_id, entry.rowset_id));
                }
                ManifestOperation::AddDV(entry) => {
                    engine
                        .next_id
                        .1
                        .fetch_max(entry.dv_id + 1, std::sync::atomic::Ordering::SeqCst);

                    dvs_to_open.insert(
                        (entry.table_id.table_id, entry.rowset_id, entry.dv_id),
                        entry,
                    );
                }
                ManifestOperation::DeleteDV(entry) => {
                    dvs_to_open.remove(&(entry.table_id.table_id, entry.rowset_id, entry.dv_id));
                }
                ManifestOperation::Begin | ManifestOperation::End => {}
            }
        }

        info!(
            "{} tables loaded, {} rowset loaded, {} DV loaded",
            engine.tables.read().len(),
            rowsets_to_open.len(),
            dvs_to_open.len()
        );

        let mut changeset = vec![];

        // TODO: parallel open

        let tables = engine.tables.read();

        for (_, entry) in rowsets_to_open {
            let table = tables.get(&entry.table_id).unwrap();
            let disk_rowset = DiskRowset::open(
                table.get_rowset_path(entry.rowset_id),
                table.columns.clone(),
                engine.block_cache.clone(),
                entry.rowset_id,
            )
            .await?;
            changeset.push(EpochOp::AddRowSet((entry, disk_rowset)));
        }

        for (_, entry) in dvs_to_open {
            let table = tables.get(&entry.table_id).unwrap();
            let dv = DeleteVector::open(
                entry.dv_id,
                entry.rowset_id,
                table.get_dv_path(entry.rowset_id, entry.dv_id),
            )
            .await?;
            changeset.push(EpochOp::AddDV((entry, dv)));
        }

        engine.version.commit_changes(changeset).await?;

        drop(tables);

        Ok(engine)
    }
}