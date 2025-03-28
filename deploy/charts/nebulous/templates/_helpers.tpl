{{- define "common.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/name: {{ .Chart.Name | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/instance: {{ .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end}}

{{- define "nebulous.namespace" -}}
{{- default .Release.Namespace .Values.namespaceOverride }}
{{- end }}

{{- define "nebulous.image" -}}
{{- $tag := default .Chart.AppVersion .Values.image.tag }}
{{- printf "%s:%s" .Values.image.repository $tag }}
{{- end }}

{{- define "nebulous.serviceAccountName" -}}
{{- default .Release.Name .Values.serviceAccount.name }}
{{- end }}

{{- define "nebulous.serviceName" -}}
{{- default .Release.Name .Values.service.nameOverride }}
{{- end }}

{{- define "nebulous.deploymentName" -}}
{{- printf "%s-server" .Release.Name }}
{{- end }}

{{- define "nebulous.appSelector" -}}
{{ .Release.Name }}
{{- end }}

{{- define "nebulous.localRoleName" -}}
{{- printf "%s-local-role" .Release.Name }}
{{- end }}

{{- define "headscale.name" -}}
headscale
{{- end }}

{{- define "headscale.serviceName" -}}
{{- default (include "headscale.name" .) .Values.headscale.service.nameOverride }}
{{- end }}

{{- define "headscale.namespace" -}}
{{- default (include "nebulous.namespace" .) .Values.headscale.namespaceOverride }}
{{- end }}

{{- define "headscale.host" -}}
http://{{- include "headscale.serviceName" . }}.{{- include "headscale.namespace" . }}.svc.cluster.local
{{- end }}

{{- define "postgres.name" -}}
postgres
{{- end }}

{{- define "postgres.host" -}}
{{- if .Values.postgres.create }}
{{- include "postgres.name" . }}.{{- include "nebulous.namespace" . }}.svc.cluster.local
{{- else }}
{{- required ".Values.postgres.host is required" .Values.postgres.auth.host }}
{{- end }}
{{- end }}

{{- define "redis.name" -}}
redis
{{- end }}

{{- define "redis.serviceName" -}}
{{- default (include "redis.name" .) .Values.redis.service.nameOverride }}
{{- end }}

{{- define "redis.host" -}}
{{- if .Values.redis.create }}
{{- include "redis.serviceName" . }}.{{- include "nebulous.namespace" . }}.svc.cluster.local
{{- else }}
{{- required ".Values.redis.auth.host is required" .Values.redis.auth.host }}
{{- end }}
{{- end }}
