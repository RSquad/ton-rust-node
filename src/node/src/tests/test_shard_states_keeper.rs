use super::*;
use crate::{
    engine_traits::EngineOperations,
    test_helper::{init_test_log, TestEngine},
};

#[ignore]
#[tokio::test]
async fn test_pss_fast_saving() -> Result<()> {
    init_test_log();
    let db_path = "/db";
    let res_path = "./";

    let engine = Arc::new(TestEngine::new_db_dir(db_path, Some(res_path)).await?);

    let last_id = engine.load_shard_client_mc_block_id()?.unwrap();
    let last_mc_state = engine.load_state(&last_id).await?;
    let db = engine.db.clone();

    // Build PssStorerPrevStuff (same as PssStorer::new does)
    let mc_state = engine.load_state(&last_id).await?;
    let split_depth = mc_state.config_params()?.base_workchain()?.persistent_state_split_depth;
    let prev_blocks_dict = &mc_state.shard_state_extra()?.prev_blocks;
    let (prev_handle, prev_top_blocks) =
        find_prev_pss(&db, engine.load_block_handle(&last_id)?.unwrap(), prev_blocks_dict)
            .await?
            .expect("No prev PSS found");

    let prev_block = db.load_block_data(&prev_handle).await?;
    let prev_split_depth =
        prev_block.read_config_params()?.base_workchain()?.persistent_state_split_depth;

    let db_ = db.clone();
    let load_cell = Arc::new(move |h: &UInt256| db_.load_cell(h));
    let cells_cache = CellsCache::new(load_cell, 50_000_000);

    let mut prev_stuff = PssStorerPrevStuff {
        mc_id: prev_handle.id().clone(),
        top_blocks: prev_top_blocks,
        split_depth: prev_split_depth,
        cells_cache,
        cached_states: HashSet::new(),
        prev_part_max_size: 5 * 1024 * 1024 * 1024,
    };

    // Get shard state and compute part roots
    let last_shard_id = last_mc_state.top_blocks(0)?.remove(0);
    let last_shard_state = engine.load_state(&last_shard_id).await?;
    let shard_handle = engine.load_block_handle(&last_shard_id)?.unwrap();
    log::info!("Last shard state: {}", last_shard_state.block_id());

    let mut part_prefixes = Vec::new();
    calc_pss_split_parts(last_shard_id.shard().clone(), split_depth, &mut part_prefixes)?;

    let abort = Arc::new(|| false) as Arc<dyn Fn() -> bool + Send + Sync>;

    for part_prefix in &part_prefixes {
        let root = last_shard_state
            .state()?
            .read_accounts()?
            .subtree_with_prefix(&part_prefix.shard_key(false), &mut 0)?
            .write_to_new_cell()?
            .into_cell()?;

        let part_id =
            PersistentStatePartId::Part(last_shard_id.clone(), part_prefix.shard_prefix_with_tag());

        PssStorer::store_one_part_fast(
            &mut prev_stuff,
            &db,
            &shard_handle,
            &part_id,
            root,
            abort.clone(),
        )
        .await?;
    }

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_pss_storer() -> Result<()> {
    init_test_log();
    let db_path = "/db";
    let res_path = "./";

    let engine = Arc::new(TestEngine::new_db_dir(db_path, Some(res_path)).await?);

    let id = engine.load_last_applied_mc_block_id()?.unwrap();
    let handle = engine.load_block_handle(&id)?.unwrap();

    let storer = PssStorer::new(
        engine.db.clone(),
        engine as Arc<dyn EngineOperations>,
        handle.clone(),
        50_000_000,
        5 * 1024 * 1024 * 1024, // 5 GB
    )
    .await?;
    storer.store().await?;

    println!("Press Enter to exit...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}
