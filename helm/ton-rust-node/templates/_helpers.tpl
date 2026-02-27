{{/*
Chart name.
*/}}
{{- define "node.name" -}}
node
{{- end }}

{{/*
Fullname: based on release name, truncated to 63 chars.
*/}}
{{- define "node.fullname" -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Chart label value.
*/}}
{{- define "node.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels.
*/}}
{{- define "node.labels" -}}
{{ include "node.selectorLabels" . }}
helm.sh/chart: {{ include "node.chart" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels (used in matchLabels and service selectors).
*/}}
{{- define "node.selectorLabels" -}}
app.kubernetes.io/name: {{ include "node.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Resolve resource names: use existing*Name if set, otherwise use the chart-managed name.
*/}}
{{- define "node.globalConfigMapName" -}}
{{- if .Values.existingGlobalConfigMapName -}}
  {{- .Values.existingGlobalConfigMapName -}}
{{- else -}}
  {{- include "node.fullname" . -}}-global-config
{{- end -}}
{{- end }}

{{- define "node.logsConfigMapName" -}}
{{- if .Values.existingLogsConfigMapName -}}
  {{- .Values.existingLogsConfigMapName -}}
{{- else -}}
  {{- include "node.fullname" . -}}-logs-config
{{- end -}}
{{- end }}

{{- define "node.nodeConfigsSecretName" -}}
{{- if .Values.existingNodeConfigsSecretName -}}
  {{- .Values.existingNodeConfigsSecretName -}}
{{- else -}}
  {{- include "node.fullname" . -}}-node-configs
{{- end -}}
{{- end }}

{{- define "node.basestateConfigMapName" -}}
{{- if .Values.existingBasestateConfigMapName -}}
  {{- .Values.existingBasestateConfigMapName -}}
{{- else -}}
  {{- include "node.fullname" . -}}-basestate
{{- end -}}
{{- end }}

{{- define "node.zerostateConfigMapName" -}}
{{- if .Values.existingZerostateConfigMapName -}}
  {{- .Values.existingZerostateConfigMapName -}}
{{- else -}}
  {{- include "node.fullname" . -}}-zerostate
{{- end -}}
{{- end }}

{{/*
ServiceAccount name: use serviceAccount.name if set, otherwise fall back to fullname.
*/}}
{{- define "node.serviceAccountName" -}}
{{- if .Values.serviceAccount.name -}}
  {{- .Values.serviceAccount.name -}}
{{- else -}}
  {{- include "node.fullname" . -}}
{{- end -}}
{{- end }}

{{/*
Boolean helpers: whether basestate/zerostate are enabled (either inline or external).
*/}}
{{- define "node.hasBasestate" -}}
{{- or .Values.basestate .Values.existingBasestateConfigMapName -}}
{{- end }}

{{- define "node.hasZerostate" -}}
{{- or .Values.zerostate .Values.existingZerostateConfigMapName -}}
{{- end }}

{{/*
Validation: require nodeConfigs or existingNodeConfigsSecretName.
*/}}
{{- define "node.validateNodeConfigs" -}}
{{- if not (or .Values.nodeConfigs .Values.existingNodeConfigsSecretName) -}}
  {{- fail "nodeConfigs is required: set nodeConfigs (inline JSON map), use --set-file nodeConfigs.node-0\\.json=path, or provide existingNodeConfigsSecretName" -}}
{{- end -}}
{{- end }}

{{/*
Merge service annotations: shared annotations from svcConfig.annotations
plus per-replica overrides from svcConfig.perReplica[replicaIndex].annotations.
Per-replica annotations win on conflict.
Usage: {{ include "node.serviceAnnotations" (dict "svcConfig" .Values.services.adnl "replicaIndex" $i) }}
*/}}
{{- define "node.serviceAnnotations" -}}
{{- $svcConfig := .svcConfig -}}
{{- $i := int .replicaIndex -}}
{{- $shared := $svcConfig.annotations | default dict -}}
{{- $perReplica := dict -}}
{{- if and (hasKey $svcConfig "perReplica") $svcConfig.perReplica (lt $i (len $svcConfig.perReplica)) -}}
{{-   $perReplica = (index $svcConfig.perReplica $i).annotations | default dict -}}
{{- end -}}
{{- $merged := mustMergeOverwrite (deepCopy $shared) $perReplica -}}
{{- if $merged }}
annotations:
  {{- $merged | toYaml | nindent 2 }}
{{- end -}}
{{- end }}

{{/*
Merge service labels: shared labels from svcConfig.labels
plus per-replica overrides from svcConfig.perReplica[replicaIndex].labels.
Per-replica labels win on conflict.
Usage: {{ include "node.serviceLabels" (dict "svcConfig" .Values.services.adnl "replicaIndex" $i) }}
*/}}
{{- define "node.serviceLabels" -}}
{{- $svcConfig := .svcConfig -}}
{{- $i := int .replicaIndex -}}
{{- $shared := $svcConfig.labels | default dict -}}
{{- $perReplica := dict -}}
{{- if and (hasKey $svcConfig "perReplica") $svcConfig.perReplica (lt $i (len $svcConfig.perReplica)) -}}
{{-   $perReplica = (index $svcConfig.perReplica $i).labels | default dict -}}
{{- end -}}
{{- $merged := mustMergeOverwrite (deepCopy $shared) $perReplica -}}
{{- if $merged }}
{{- $merged | toYaml }}
{{- end -}}
{{- end }}

{{/*
Boolean helper: whether vault is configured.
*/}}
{{- define "node.hasVault" -}}
{{- or .Values.vault.url .Values.vault.secretName -}}
{{- end }}

{{/*
Vault env var block. Renders VAULT_URL when vault is configured.
Usage: {{ include "node.vaultEnv" . | nindent N }}
*/}}
{{- define "node.vaultEnv" -}}
{{- if or .Values.vault.url .Values.vault.secretName -}}
- name: VAULT_URL
  {{- if .Values.vault.secretName }}
  valueFrom:
    secretKeyRef:
      name: {{ .Values.vault.secretName }}
      key: {{ .Values.vault.secretKey | default "VAULT_URL" }}
  {{- else }}
  value: {{ .Values.vault.url | quote }}
  {{- end }}
{{- end -}}
{{- end }}

{{/*
Config checksum — SHA-256 of all configuration data concatenated.
Forces pod restart when any config changes.
For globalConfig and logsConfig, includes bundled file content when no inline
value is provided (mirrors the configmap template logic).
External resources (existing*Name) are excluded — they are managed outside the chart.
*/}}
{{- define "node.configChecksum" -}}
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
