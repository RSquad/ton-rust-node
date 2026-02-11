import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultTimeseries, promQuery, F } from "./common";

export const enqueueDequeueRate = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Outqueue Enqueue/Dequeue Rate")
    .unit(units.OpsPerSecond)
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_enqueued_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} enqueued/s",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_dequeued_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} dequeued/s",
      ),
    );

export const cleanDuration = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Outqueue Clean Duration")
    .unit(units.Seconds)
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_outqueue_clean_duration_seconds{${F}})`,
        "{{node_id}} {{shard}}",
      ),
    );

export const cleanStats = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Outqueue Clean Stats")
    .fillOpacity(5)
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_outqueue_clean_processed{${F}})`,
        "{{node_id}} processed",
      ),
    )
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_outqueue_clean_deleted{${F}})`,
        "{{node_id}} deleted",
      ),
    )
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_outqueue_clean_partial{${F}})`,
        "{{node_id}} partial",
      ),
    );
