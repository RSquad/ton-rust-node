import * as common from "@grafana/grafana-foundation-sdk/common";
import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultTimeseries, promQuery, histogramP, F } from "./common";

export const activeValidatorsCollators = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Active Validators & Collators")
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_validator_active{${F}})`,
        "{{node_id}} validators",
      ),
    )
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_collator_active{${F}})`,
        "{{node_id}} collators",
      ),
    );

export const validationResults = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Validation Results (rate)")
    .unit(units.OpsPerSecond)
    .stacking(
      new common.StackingConfigBuilder().mode(common.StackingMode.Normal),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_validator_successes_total{${F}}[$__rate_interval]))`,
        "{{node_id}} success/s",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_validator_failures_total{${F}}[$__rate_interval]))`,
        "{{node_id}} fail/s",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_validator_ref_block_failures_total{${F}}[$__rate_interval]))`,
        "{{node_id}} ref block fail/s",
      ),
    );

export const collationDuration = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Collation Duration")
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.5,
        "ton_node_collator_duration_seconds_bucket",
        "{{node_id}} p50",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_collator_duration_seconds_bucket",
        "{{node_id}} p95",
      ),
    )
    .withTarget(
      histogramP(
        0.99,
        "ton_node_collator_duration_seconds_bucket",
        "{{node_id}} p99",
      ),
    );

export const collationResults = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Collation Results (rate)")
    .unit(units.OpsPerSecond)
    .stacking(
      new common.StackingConfigBuilder().mode(common.StackingMode.Normal),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_successes_total{${F}}[$__rate_interval]))`,
        "{{node_id}} success/s",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_failures_total{${F}}[$__rate_interval]))`,
        "{{node_id}} fail/s",
      ),
    );

export const collationGasUsed = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Collation Gas Used")
    .fillOpacity(5)
    .withTarget(
      histogramP(
        0.5,
        "ton_node_collator_gas_used_bucket",
        "{{node_id}} p50",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_collator_gas_used_bucket",
        "{{node_id}} p95",
      ),
    );

export const collationMessageThroughput = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Collation Message Throughput (rate)")
    .unit(units.OpsPerSecond)
    .drawStyle(common.GraphDrawStyle.Bars)
    .fillOpacity(50)
    .stacking(
      new common.StackingConfigBuilder().mode(common.StackingMode.Normal),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_inbound_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} inbound",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_outbound_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} outbound",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_transit_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} transit",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_executed_transactions_total{${F}}[$__rate_interval]))`,
        "{{node_id}} transactions",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_enqueued_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} enqueued",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_collator_dequeued_messages_total{${F}}[$__rate_interval]))`,
        "{{node_id}} dequeued",
      ),
    );

export const collationInternalTimings = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Collation Internal Timings (p95)")
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.95,
        "ton_node_collator_process_ext_messages_seconds_bucket",
        "{{node_id}} ext msgs",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_collator_process_new_messages_seconds_bucket",
        "{{node_id}} new msgs",
      ),
    );
