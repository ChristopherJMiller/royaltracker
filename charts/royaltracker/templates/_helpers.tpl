{{/*
Common labels.
*/}}
{{- define "royaltracker.labels" -}}
app.kubernetes.io/name: royaltracker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{- define "royaltracker.selectorLabels" -}}
app.kubernetes.io/name: royaltracker
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/*
Shared env block: pulls config from configSecret and database from database.secretName.
*/}}
{{- define "royaltracker.env" -}}
- name: ROYALTRACKER_TELEGRAM__BOT_TOKEN
  valueFrom: { secretKeyRef: { name: {{ .Values.configSecret.name }}, key: bot_token } }
- name: ROYALTRACKER_ENCRYPTION_KEY_B64
  valueFrom: { secretKeyRef: { name: {{ .Values.configSecret.name }}, key: encryption_key_b64 } }
- name: ROYALTRACKER_RCG_BASIC_AUTH_B64
  valueFrom: { secretKeyRef: { name: {{ .Values.configSecret.name }}, key: rcg_basic_auth_b64 } }
- name: ROYALTRACKER_DATABASE_URL
  valueFrom: { secretKeyRef: { name: {{ .Values.database.secretName }}, key: {{ .Values.database.secretKey }} } }
- name: ROYALTRACKER_WEB__PUBLIC_URL
  value: {{ required "publicUrl is required (the Mini App's HTTPS URL)" .Values.publicUrl | quote }}
- name: ROYALTRACKER_WEB__BIND_ADDR
  value: "0.0.0.0:{{ .Values.bot.port }}"
- name: ROYALTRACKER_TELEGRAM__ADMIN_CHAT_ID
  value: {{ .Values.adminChatId | quote }}
- name: ROYALTRACKER_JITTER_MINUTES
  value: {{ .Values.jitterMinutes | quote }}
- name: RUST_LOG
  value: "{{ .Values.logLevel }},royaltracker={{ .Values.logLevel }}"
{{- end -}}
