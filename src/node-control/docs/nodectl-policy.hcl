path "transit/keys/*" {
  capabilities = [ "read", "update" ]
}

path "transit/sign/*" {
  capabilities = [ "update" ]
}

path "transit/verify/*" {
  capabilities = [ "update" ]
}

path "transit/export/encryption-key/*" {
  capabilities = [ "read" ]
}

path "transit/export/signing-key/*" {
  capabilities = [ "read" ]
}

path "transit/wrapping_key" {
  capabilities = [ "read" ]
}

path "secret/data/transit-metadata/*" {
  capabilities = [ "read" ]
}
