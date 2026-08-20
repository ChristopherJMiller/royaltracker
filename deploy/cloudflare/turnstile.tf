###############################################################################
# Turnstile — bot protection for the public signup action.
#
# "managed" mode lets Cloudflare decide between an invisible pass and an
# interactive challenge. The widget yields a public SITE KEY (embedded in the
# signup page) and a SECRET (server-side siteverify). The secret is created
# here but consumed later by the web app via a k8s Secret in luma-homeops.
###############################################################################

resource "cloudflare_turnstile_widget" "signup" {
  account_id = var.account_id
  name       = var.turnstile_widget_name
  domains    = var.turnstile_domains
  # "managed" lets Cloudflare pick invisible-vs-interactive — the friction/
  # security sweet spot vs. always-interactive.
  mode = "managed"
}
