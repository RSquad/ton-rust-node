import * as common from "@grafana/grafana-foundation-sdk/common";
import * as dashboard from "@grafana/grafana-foundation-sdk/dashboard";
import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultTimeseries, promQuery, F } from "./common";

export const mcTimediff = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("MC Timediff")
    .unit(units.Seconds)
    .gradientMode(common.GraphGradientMode.Scheme)
    .thresholds(
      new dashboard.ThresholdsConfigBuilder()
        .mode(dashboard.ThresholdsMode.Absolute)
        .steps([
          { value: null, color: "green" },
          { value: 30, color: "yellow" },
          { value: 120, color: "red" },
        ]),
    )
    .thresholdsStyle(
      new common.GraphThresholdsStyleConfigBuilder().mode(
        common.GraphThresholdsStyleMode.Line,
      ),
    )
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_engine_timediff_seconds{${F}})`,
        "{{node_id}}",
      ),
    );

export const mcBlockSeqno = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("MC Block Seqno")
    .fillOpacity(0)
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_engine_last_mc_block_seqno{${F}})`,
        "{{node_id}}",
      ),
    );

export const extMessagesQueue = (): TimeseriesBuilder =>
  defaultTimeseries()
    .title("External Messages Queue")
    .fillOpacity(5)
    .withTarget(
      promQuery(
        `max by(node_id) (ton_node_ext_messages_queue_size{${F}})`,
        "{{node_id}} queue size",
      ),
    )
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_ext_messages_expired_total{${F}}[$__rate_interval]))`,
        "{{node_id}} expired/s",
      ),
    );
