import {
  DashboardBuilder,
  DashboardCursorSync,
  RowBuilder,
} from "@grafana/grafana-foundation-sdk/dashboard";
import { queryVariable } from "./common";
import * as status from "./status";
import * as tps from "./tps";
import * as sync from "./sync";
import * as validation from "./validation";
import * as outqueue from "./outqueue";
import * as network from "./network";
import * as database from "./database";

const builder = new DashboardBuilder("TON Node Overview")
  .uid("ton-node-overview")
  .tags(["ton", "node", "blockchain"])
  .editable()
  .tooltip(DashboardCursorSync.Crosshair)
  .refresh("30s")
  .time({ from: "now-1h", to: "now" })
  .timezone("browser")

  // Template variables
  .withVariable(
    queryVariable(
      "network",
      "Network",
      "label_values(ton_node_engine_sync_status, network)",
    ),
  )
  .withVariable(
    queryVariable(
      "node_id",
      "Node ID",
      'label_values(ton_node_engine_sync_status{network=~"$network"}, node_id)',
    ),
  )

  // Node Status & Build Info
  .withPanel(status.nodeStatusTable())
  .withPanel(status.buildInfoTable())

  // TPS
  .withRow(new RowBuilder("TPS"))
  .withPanel(tps.tps10s())
  .withPanel(tps.tps5m())
  .withPanel(tps.tps30m())

  // Sync & Block Progress
  .withRow(new RowBuilder("Sync & Block Progress"))
  .withPanel(sync.mcTimediff().span(12).height(8))
  .withPanel(sync.mcBlockSeqno().span(12).height(8))
  .withPanel(sync.extMessagesQueue().span(12).height(8))

  // Validation & Collation
  .withRow(new RowBuilder("Validation & Collation"))
  .withPanel(validation.activeValidatorsCollators().span(12).height(8))
  .withPanel(validation.validationResults().span(12).height(8))
  .withPanel(validation.collationDuration().span(12).height(8))
  .withPanel(validation.collationResults().span(12).height(8))
  .withPanel(validation.collationGasUsed().span(12).height(8))
  .withPanel(validation.collationMessageThroughput().span(12).height(8))
  .withPanel(validation.collationInternalTimings().span(12).height(8))

  // Outbound Message Queue
  .withRow(new RowBuilder("Outbound Message Queue"))
  .withPanel(outqueue.enqueueDequeueRate().span(12).height(8))
  .withPanel(outqueue.cleanDuration().span(12).height(8))
  .withPanel(outqueue.cleanStats().span(12).height(8))

  // Network
  .withRow(new RowBuilder("Network"))
  .withPanel(network.adnlRoundtrip().span(12).height(8))
  .withPanel(network.catchainQueryTimes().span(12).height(8))
  .withPanel(network.neighbourFailures().span(12).height(8))
  .withPanel(network.consensusOverlayQueryTime().span(12).height(8))
  .withPanel(network.neighbourUnreliability().span(12).height(8))

  // Database & Storage
  .withRow(new RowBuilder("Database & Storage"))
  .withPanel(database.shardStateQueueSize().span(12).height(8))
  .withPanel(database.dbOperationDurations().span(12).height(8));

// Build and prepend Grafana import metadata.
// __inputs makes Grafana prompt for datasource selection on import.
const dashboard = {
  __inputs: [
    {
      name: "DS_PROMETHEUS",
      label: "Prometheus",
      description: "",
      type: "datasource",
      pluginId: "prometheus",
      pluginName: "Prometheus",
    },
  ],
  __requires: [
    { type: "grafana", id: "grafana", name: "Grafana", version: "10.0.0" },
    {
      type: "datasource",
      id: "prometheus",
      name: "Prometheus",
      version: "1.0.0",
    },
    { type: "panel", id: "stat", name: "Stat", version: "" },
    { type: "panel", id: "timeseries", name: "Time series", version: "" },
    { type: "panel", id: "table", name: "Table", version: "" },
  ],
  __elements: {},
  ...builder.build(),
};

console.log(JSON.stringify(dashboard, null, 2));
