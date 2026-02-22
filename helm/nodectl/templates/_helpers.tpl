{{/*
Chart name.
*/}}
{{- define "nodectl.name" -}}
nodectl
{{- end }}

{{/*
Fullname: based on release name, truncated to 63 chars.
*/}}
{{- define "nodectl.fullname" -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Chart label value.
*/}}
{{- define "nodectl.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels.
*/}}
{{- define "nodectl.labels" -}}
{{ include "nodectl.selectorLabels" . }}
helm.sh/chart: {{ include "nodectl.chart" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels (used in matchLabels and service selectors).
*/}}
{{- define "nodectl.selectorLabels" -}}
app.kubernetes.io/name: {{ include "nodectl.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
ServiceAccount name: use serviceAccount.name if set, otherwise fall back to fullname.
*/}}
{{- define "nodectl.serviceAccountName" -}}
{{- if .Values.serviceAccount.name -}}
  {{- .Values.serviceAccount.name -}}
{{- else -}}
  {{- include "nodectl.fullname" . -}}
{{- end -}}
{{- end }}
