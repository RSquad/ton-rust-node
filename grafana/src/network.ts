import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultTimeseries, promQuery, histogramP, F } from "./common";

export const adnlRoundtrip = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("ADNL Roundtrip")
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.5,
        "ton_node_network_adnl_roundtrip_seconds_bucket",
        "{{node_id}} p50",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_network_adnl_roundtrip_seconds_bucket",
        "{{node_id}} p95",
      ),
    );

export const catchainQueryTimes = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Catchain Query Times")
    .description(
      "Only available when catchain consensus is used. Empty on simplex networks.",
    )
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.95,
        "ton_node_network_catchain_overlay_query_seconds_bucket",
        "{{node_id}} overlay p95",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_network_catchain_send_seconds_bucket",
        "{{node_id}} send p95",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_network_catchain_client_query_seconds_bucket",
        "{{node_id}} client p95",
      ),
    );

export const neighbourFailures = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Neighbour Failures (rate)")
    .unit(units.OpsPerSecond)
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_network_neighbour_failures_total{${F}}[$__rate_interval]))`,
        "{{node_id}}",
      ),
    );

export const consensusOverlayQueryTime = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Consensus Overlay Query Time")
    .unit(units.Seconds)
    .withTarget(
      histogramP(
        0.5,
        "ton_node_network_consensus_overlay_query_seconds_bucket",
        "{{node_id}} p50",
      ),
    )
    .withTarget(
      histogramP(
        0.95,
        "ton_node_network_consensus_overlay_query_seconds_bucket",
        "{{node_id}} p95",
      ),
    );

export const neighbourUnreliability = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("Neighbour Unreliability Score")
    .fillOpacity(5)
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_network_neighbour_unreliability{${F}})`,
        "{{node_id}} max",
      ),
    )
    .withTarget(
      promQuery(
        `avg by(node_id) (ton_node_network_neighbour_unreliability{${F}})`,
        "{{node_id}} avg",
      ),
    );
