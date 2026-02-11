import * as common from "@grafana/grafana-foundation-sdk/common";
import * as prometheus from "@grafana/grafana-foundation-sdk/prometheus";
import * as dashboard from "@grafana/grafana-foundation-sdk/dashboard";
import { PanelBuilder as TimeseriesBuilder } from "@grafana/grafana-foundation-sdk/timeseries";
import { PanelBuilder as StatBuilder } from "@grafana/grafana-foundation-sdk/stat";

// Shared datasource reference
export const DS: common.DataSourceRef = {
  type: "prometheus",
  uid: "${DS_PROMETHEUS}",
};

// Label filter used across all queries
export const F = 'network=~"$network", node_id=~"$node_id"';

// --- Query helpers ---

export const promQuery = (
  expr: string,
  legend: string,
): prometheus.DataqueryBuilder =>
  new prometheus.DataqueryBuilder()
    .datasource(DS)
    .expr(expr)
    .legendFormat(legend);

export const promInstant = (
  expr: string,
  refId: string,
): prometheus.DataqueryBuilder =>
  new prometheus.DataqueryBuilder()
    .datasource(DS)
    .expr(expr)
    .format(prometheus.PromQueryFormat.Table)
    .instant()
    .refId(refId);

/** histogram_quantile wrapper */
export const histogramP = (
  quantile: number,
  metric: string,
  legend: string,
): prometheus.DataqueryBuilder =>
  promQuery(
    `histogram_quantile(${quantile}, sum by(node_id, le) (rate(${metric}{${F}}[$__rate_interval])))`,
    legend,
  );

// --- Template variable helper ---

export const queryVariable = (
  name: string,
  label: string,
  query: string,
): dashboard.QueryVariableBuilder =>
  new dashboard.QueryVariableBuilder(name)
    .label(label)
    .query(query)
    .datasource(DS)
    .current({ selected: false, text: "All", value: "$__all" })
    .refresh(dashboard.VariableRefresh.OnTimeRangeChanged)
    .multi(true)
    .includeAll(true);

// --- Default panel builders ---

export const defaultTimeseries = (): TimeseriesBuilder =>
  new TimeseriesBuilder()
    .datasource(DS)
    .lineWidth(1)
    .fillOpacity(10)
    .drawStyle(common.GraphDrawStyle.Line)
    .legend(
      new common.VizLegendOptionsBuilder()
        .showLegend(true)
        .placement(common.LegendPlacement.Bottom)
        .displayMode(common.LegendDisplayMode.List),
    )
    .tooltip(
      new common.VizTooltipOptionsBuilder().mode(
        common.TooltipDisplayMode.Multi,
      ),
    );

export const defaultStat = (): StatBuilder =>
  new StatBuilder()
    .datasource(DS)
    .span(8)
    .height(4)
    .reduceOptions(
      new common.ReduceDataOptionsBuilder().calcs(["lastNotNull"]),
    )
    .colorMode(common.BigValueColorMode.Background)
    .graphMode(common.BigValueGraphMode.Area);
