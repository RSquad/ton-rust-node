import * as common from "@grafana/grafana-foundation-sdk/common";
import { PanelBuilder as TableBuilder } from "@grafana/grafana-foundation-sdk/table";
import { DS, F, promInstant } from "./common";

export const nodeStatusTable = (): TableBuilder =>
  new TableBuilder()
    .title("Node Status")
    .datasource(DS)
    .span(24)
    .height(7)
    .showHeader(true)
    .cellHeight(common.TableCellHeight.Sm)
    .sortBy([
      new common.TableSortByFieldStateBuilder()
        .displayName("Node")
        .desc(false),
    ])
    .withTarget(
      promInstant(
        `max by(node_id) (ton_node_engine_sync_status{${F}})`,
        "A",
      ),
    )
    .withTarget(
      promInstant(`max by(node_id) (ton_node_validator_status{${F}})`, "B"),
    )
    .withTarget(
      promInstant(
        `max by(node_id) (ton_node_engine_timediff_seconds{${F}})`,
        "C",
      ),
    )
    .withTarget(
      promInstant(
        `clamp_max(max by(node_id) (ton_node_engine_will_validate{${F}}) + max by(node_id) (ton_node_validator_in_next_set{${F}}), 1)`,
        "D",
      ),
    )
    .withTarget(
      promInstant(
        `max by(node_id) (ton_node_engine_last_mc_block_utime{${F}}) * 1000`,
        "E",
      ),
    )
    .withTransformation({
      id: "joinByField",
      options: { byField: "node_id", mode: "outer" },
    })
    .withTransformation({
      id: "organize",
      options: {
        excludeByName: {
          Time: true,
          "Time 1": true,
          "Time 2": true,
          "Time 3": true,
          "Time 4": true,
          "Time 5": true,
        },
        indexByName: {
          node_id: 0,
          "Value #A": 1,
          "Value #B": 2,
          "Value #C": 3,
          "Value #D": 4,
          "Value #E": 5,
        },
        renameByName: {
          node_id: "Node",
          "Value #A": "Sync Status",
          "Value #B": "Validation Status",
          "Value #C": "MC Timediff",
          "Value #D": "Next Round",
          "Value #E": "MC Block Time",
        },
      },
    })
    .overrideByName("Sync Status", [
      {
        id: "mappings",
        value: [
          { type: "value", options: { "0": { text: "Not Set", color: "orange" } } },
          { type: "value", options: { "1": { text: "Booting", color: "yellow" } } },
          { type: "value", options: { "3": { text: "Loading States", color: "yellow" } } },
          { type: "value", options: { "4": { text: "Finishing Boot", color: "yellow" } } },
          { type: "value", options: { "5": { text: "Syncing", color: "yellow" } } },
          { type: "value", options: { "6": { text: "Synced", color: "green" } } },
          { type: "value", options: { "7": { text: "Checking DB", color: "orange" } } },
          { type: "value", options: { "8": { text: "DB Broken", color: "red" } } },
        ],
      },
      {
        id: "custom.cellOptions",
        value: { type: "color-background", mode: "basic" },
      },
    ])
    .overrideByName("Validation Status", [
      {
        id: "mappings",
        value: [
          { type: "value", options: { "0": { text: "Not in Set", color: "text" } } },
          { type: "value", options: { "1": { text: "Waiting", color: "yellow" } } },
          { type: "value", options: { "2": { text: "Countdown", color: "orange" } } },
          { type: "value", options: { "3": { text: "Active", color: "green" } } },
        ],
      },
      {
        id: "custom.cellOptions",
        value: { type: "color-background", mode: "basic" },
      },
    ])
    .overrideByName("MC Timediff", [
      { id: "unit", value: "s" },
      {
        id: "thresholds",
        value: {
          mode: "absolute",
          steps: [
            { value: null, color: "green" },
            { value: 20, color: "yellow" },
            { value: 60, color: "red" },
          ],
        },
      },
      {
        id: "custom.cellOptions",
        value: { type: "color-background", mode: "basic" },
      },
    ])
    .overrideByName("Next Round", [
      {
        id: "mappings",
        value: [
          { type: "value", options: { "0": { text: "No", color: "text" } } },
          { type: "value", options: { "1": { text: "Yes", color: "green" } } },
        ],
      },
      {
        id: "custom.cellOptions",
        value: { type: "color-background", mode: "basic" },
      },
    ])
    .overrideByName("MC Block Time", [
      { id: "unit", value: "dateTimeFromNow" },
    ]);

export const buildInfoTable = (): TableBuilder =>
  new TableBuilder()
    .title("Build Info")
    .datasource(DS)
    .span(24)
    .height(7)
    .showHeader(true)
    .cellHeight(common.TableCellHeight.Sm)
    .withTarget(promInstant(`ton_node_build_info{${F}}`, "A"))
    .withTransformation({ id: "labelsToFields", options: {} })
    .withTransformation({
      id: "organize",
      options: {
        excludeByName: {
          Time: true,
          Value: true,
          __name__: true,
          arch: true,
          container: true,
          endpoint: true,
          instance: true,
          job: true,
          namespace: true,
          os: true,
          service: true,
          pod: true,
        },
        indexByName: {
          node_id: 0,
          network: 1,
          version: 2,
          commit: 3,
          rustversion: 4,
          branch: 5,
          build_time: 6,
        },
        renameByName: {
          node_id: "Node",
          network: "Network",
          version: "Version",
          commit: "Commit",
          rustversion: "Rust",
          branch: "Branch",
          build_time: "Build Time",
        },
      },
    });
