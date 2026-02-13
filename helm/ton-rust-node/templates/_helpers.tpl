{{/*
Chart name.
*/}}
{{- define "ton-rust-node.name" -}}
ton-rust-node
{{- end }}

{{/*
Fullname: based on release name, truncated to 63 chars.
*/}}
{{- define "ton-rust-node.fullname" -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Chart label value.
*/}}
{{- define "ton-rust-node.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels.
*/}}
{{- define "ton-rust-node.labels" -}}
{{ include "ton-rust-node.selectorLabels" . }}
helm.sh/chart: {{ include "ton-rust-node.chart" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels (used in matchLabels and service selectors).
*/}}
{{- define "ton-rust-node.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ton-rust-node.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Resolve resource names: use existing*Name if set, otherwise use the chart-managed name.
*/}}
{{- define "ton-rust-node.globalConfigMapName" -}}
{{- if .Values.existingGlobalConfigMapName -}}
  {{- .Values.existingGlobalConfigMapName -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}-global-config
{{- end -}}
{{- end }}

{{- define "ton-rust-node.logsConfigMapName" -}}
{{- if .Values.existingLogsConfigMapName -}}
  {{- .Values.existingLogsConfigMapName -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}-logs-config
{{- end -}}
{{- end }}

{{- define "ton-rust-node.nodeConfigsSecretName" -}}
{{- if .Values.existingNodeConfigsSecretName -}}
  {{- .Values.existingNodeConfigsSecretName -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}-node-configs
{{- end -}}
{{- end }}

{{- define "ton-rust-node.basestateConfigMapName" -}}
{{- if .Values.existingBasestateConfigMapName -}}
  {{- .Values.existingBasestateConfigMapName -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}-basestate
{{- end -}}
{{- end }}

{{- define "ton-rust-node.zerostateConfigMapName" -}}
{{- if .Values.existingZerostateConfigMapName -}}
  {{- .Values.existingZerostateConfigMapName -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}-zerostate
{{- end -}}
{{- end }}

{{/*
ServiceAccount name: use serviceAccount.name if set, otherwise fall back to fullname.
*/}}
{{- define "ton-rust-node.serviceAccountName" -}}
{{- if .Values.serviceAccount.name -}}
  {{- .Values.serviceAccount.name -}}
{{- else -}}
  {{- include "ton-rust-node.fullname" . -}}
{{- end -}}
{{- end }}

{{/*
Boolean helpers: whether basestate/zerostate are enabled (either inline or external).
*/}}
{{- define "ton-rust-node.hasBasestate" -}}
{{- or .Values.basestate .Values.existingBasestateConfigMapName -}}
{{- end }}

{{- define "ton-rust-node.hasZerostate" -}}
{{- or .Values.zerostate .Values.existingZerostateConfigMapName -}}
{{- end }}

{{/*
Validation: require nodeConfigs or existingNodeConfigsSecretName.
*/}}
{{- define "ton-rust-node.validateNodeConfigs" -}}
{{- if not (or .Values.nodeConfigs .Values.existingNodeConfigsSecretName) -}}
  {{- fail "nodeConfigs is required: set nodeConfigs (inline JSON map), use --set-file nodeConfigs.node-0\\.json=path, or provide existingNodeConfigsSecretName" -}}
{{- end -}}
{{- end }}

{{/*
Config checksum — SHA-256 of all configuration data concatenated.
Forces pod restart when any config changes.
For globalConfig and logsConfig, includes bundled file content when no inline
value is provided (mirrors the configmap template logic).
External resources (existing*Name) are excluded — they are managed outside the chart.
*/}}
{{- define "ton-rust-node.configChecksum" -}}
{{- $globalConfig := "" -}}
{{- if not .Values.existingGlobalConfigMapName -}}
  {{- $globalConfig = default (.Files.Get "files/global.config.json") .Values.globalConfig -}}
{{- end -}}
{{- $logsConfig := "" -}}
{{- if not .Values.existingLogsConfigMapName -}}
  {{- $logsConfig = default (.Files.Get "files/logs.config.yml") .Values.logsConfig -}}
{{- end -}}
{{- $parts := list
  $globalConfig
  $logsConfig
  (default "" (toJson .Values.nodeConfigs))
  (default "" .Values.basestate)
  (default "" .Values.zerostate)
-}}
{{- join "" $parts | sha256sum }}
{{- end }}
