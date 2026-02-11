import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultTimeseries, promQuery, histogramP, F } from "./common";

export const shardStateQueueSize = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Shard State Queue Size")
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_db_shardstate_queue_size{${F}})`,
        "{{node_id}}",
      ),
    );

export const dbOperationDurations = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("DB Operation Durations (p95)")
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.95,
        "ton_node_db_shardstate_gc_seconds_bucket",
        "{{node_id}} shard GC",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_db_persistent_state_write_seconds_bucket",
        "{{node_id}} persistent write",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_db_calc_merkle_update_seconds_bucket",
        "{{node_id}} merkle calc",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_db_restore_merkle_update_seconds_bucket",
        "{{node_id}} merkle restore",
      ),
    );
