import * as dashboard from "@grafana/grafana-foundation-sdk/dashboard";
import { PanelBuilder as StatBuilder } from "@grafana/grafana-foundation-sdk/stat";
import * as units from "@grafana/grafana-foundation-sdk/units";
import { defaultStat, promQuery, F } from "./common";

const tpsStat = (title: string, window: string): StatBuilder =>
  defaultStat()
    .title(title)
    .withTarget(
      promQuery(
        `sum by(node_id) (rate(ton_node_engine_applied_transactions_total{${F}}[${window}]))`,
        "{{node_id}}",
      ),
    )
    .unit(units.OpsPerSecond)
    .decimals(1)
    .thresholds(
      new dashboard.ThresholdsConfigBuilder()
        .mode(dashboard.ThresholdsMode.Absolute)
        .steps([{ value: null, color: "blue" }]),
    );

export const tps10s = (): StatBuilder => tpsStat("TPS (10s)", "10s");
export const tps5m = (): StatBuilder => tpsStat("TPS (5m)", "5m");
export const tps30m = (): StatBuilder => tpsStat("TPS (30m)", "30m");
