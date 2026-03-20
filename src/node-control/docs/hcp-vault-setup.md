# HCP Vault Setup for nodectl

This guide walks you through setting up HashiCorp Cloud Platform (HCP) Vault Dedicated for the `nodectl` application, including creating a namespace, enabling the transit secrets engine, and configuring AppRole authentication.

---

## Step 1: Create HashiCorp Cloud Platform Account

1. Navigate to [https://portal.cloud.hashicorp.com/sign-up/](https://portal.cloud.hashicorp.com/sign-up/)
2. Choose your sign-up method:
   - **Email**: Enter your email and create a password
   - **GitHub**: Sign in with your GitHub account
   - **Google**: Sign in with your Google account
3. Verify your email address if prompted
4. Complete the onboarding questionnaire
5. You'll be redirected to the HCP Portal dashboard

---

## Step 2: Create a Vault Cluster

1. In the HCP Portal, click **Vault Dedicated** from the left navigation
2. Click **Create cluster**
3. Configure your cluster:
   - **Cluster ID**: Enter a unique name (e.g., `nodectl-vault`)
   - **Region**: Select your preferred cloud region
   - **Tier**: Select the tier that fits your needs: **Starter** / **Standard** / **Plus** . Don't use **Development** tier for production.
   - **Network**: Create a new HVN (HashiCorp Virtual Network) or select existing
4. Click **Create cluster**
5. Wait for the cluster to initialize (this may take a few minutes)

---

## Step 3: Generate Admin Token

1. Once the cluster is **Running**, click on the cluster name
2. Click **Generate token** (or **New admin token** if one was already created)
3. Copy and save the admin token. **Important:** it has a TTL equal to 6 hours
4. Note the **Public Cluster URL** from the cluster overview page

Set environment variables for later use:

```bash
export VAULT_ADDR="<Your-Public-Cluster-URL>"
export VAULT_TOKEN="<Your-Admin-Token>"
export VAULT_NAMESPACE="admin"
```

Example:

```bash
export VAULT_ADDR="https://nodectl-vault-public-vault-xxxxxxxx.hashicorp.cloud:8200"
export VAULT_TOKEN="hvs.CAESIG..."
export VAULT_NAMESPACE="admin"
```

---

## Step 4: Install Vault CLI

### macOS (Homebrew)

```bash
brew tap hashicorp/tap
brew install hashicorp/tap/vault
```

### Linux (Ubuntu/Debian)

```bash
wget -O- https://apt.releases.hashicorp.com/gpg | sudo gpg --dearmor -o /usr/share/keyrings/hashicorp-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/hashicorp-archive-keyring.gpg] https://apt.releases.hashicorp.com $(lsb_release -cs) main" | sudo tee /etc/apt/sources.list.d/hashicorp.list
sudo apt update && sudo apt install vault
```

### Linux (RHEL/CentOS/Fedora)

```bash
sudo yum install -y yum-utils
sudo yum-config-manager --add-repo https://rpm.releases.hashicorp.com/RHEL/hashicorp.repo
sudo yum -y install vault
```

### Verify Installation

```bash
vault --version
```

### Verify Connection

```bash
vault status
```

You should see output showing the Vault server is initialized and unsealed.

---

## Step 5: Create Namespace `nodectl`

Create a new namespace called `nodectl` under the `admin` namespace:

```bash
vault namespace create nodectl
```

**Expected output:**
```
Success! Namespace created at: admin/nodectl/
```

Verify the namespace was created:

```bash
vault namespace list
```

---

## Step 6: Enable Transit Secrets Engine for nodectl Namespace

Switch to the `nodectl` namespace and enable the transit secrets engine:

```bash
export VAULT_NAMESPACE="admin/nodectl"
vault secrets enable transit
```

**Expected output:**
```
Success! Enabled the transit secrets engine at: transit/
```

Verify the secrets engine is enabled:

```bash
vault secrets list
```

You should see `transit/` in the list.

---

## Step 7: Create Policy `nodectl-service-policy`

Create the `nodectl-service-policy` policy from the policy file. First, ensure you're in the correct namespace:

```bash
export VAULT_NAMESPACE="admin/nodectl"
```

Create the policy using a heredoc (or from the file `nodectl-policy.hcl`):

```bash
vault policy write nodectl-service-policy - << 'EOF'
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
EOF
```

Or from file:

```bash
vault policy write nodectl-service-policy nodectl-policy.hcl
```

**Expected output:**
```
Success! Uploaded policy: nodectl-service-policy
```

Verify the policy was created:

```bash
vault policy list
```

Read the policy to confirm its contents:

```bash
vault policy read nodectl-service-policy
```

---

## Step 8: Enable AppRole Auth Method for nodectl Namespace

Enable the AppRole authentication method in the `nodectl` namespace:

```bash
export VAULT_NAMESPACE="admin/nodectl"
vault auth enable approle
```

**Expected output:**
```
Success! Enabled approle auth method at: approle/
```

Verify the auth method is enabled:

```bash
vault auth list
```

---

## Step 9: Create nodectl Service Role

Create an AppRole role named `nodectl-app` with the `nodectl-service-policy` policy attached:

```bash
export VAULT_NAMESPACE="admin/nodectl"
vault write auth/approle/role/nodectl-app \
    token_policies="nodectl-service-policy" \
    token_ttl=8760h
```

**Expected output:**
```
Success! Data written to: auth/approle/role/nodectl-app
```

### Get Role ID

Retrieve the RoleID for the `nodectl-app` role:

```bash
vault read auth/approle/role/nodectl-app/role-id
```

**Example output:**
```
Key     Value
---     -----
role_id 675a50e7-cfe0-be76-e35f-49ec009731ea
```

Save the role_id:

```bash
export ROLE_ID=$(vault read -field=role_id auth/approle/role/nodectl-app/role-id)
echo "Role ID: $ROLE_ID"
```

### Generate Secret ID

Generate a SecretID for the `nodectl-app` role:

```bash
vault write -force auth/approle/role/nodectl-app/secret-id
```

**Example output:**
```
Key                 Value
---                 -----
secret_id           ed0a642f-2acf-c2da-232f-1b21300d5f29
secret_id_accessor  a240a31f-270a-4765-64bd-94ba1f65703c
secret_id_ttl       0
```

Save the secret_id:

```bash
export SECRET_ID=$(vault write -force -field=secret_id auth/approle/role/nodectl-app/secret-id)
echo "Secret ID: $SECRET_ID"
```

---

## Step 10: Login with AppRole

Authenticate using the RoleID and SecretID to obtain a client token:

```bash
export VAULT_NAMESPACE="admin/nodectl"
vault write auth/approle/login \
    role_id="$ROLE_ID" \
    secret_id="$SECRET_ID"
```

**Example output:**
```
Key                     Value
---                     -----
token                   hvs.CAESIGxyz...
token_accessor          abc123...
token_duration          1h
token_renewable         true
token_policies          ["default" "nodectl-service-policy"]
identity_policies       []
policies                ["default" "nodectl-service-policy"]
token_meta_role_name    nodectl-app
```

Save the token directly:

```bash
export NODECTL_TOKEN=$(vault write -field=token auth/approle/login \
    role_id="$ROLE_ID" \
    secret_id="$SECRET_ID")
echo "NODECTL_TOKEN: $NODECTL_TOKEN"
```

---

## Step 11: Setup Environment Variable

Export the `NODECTL_TOKEN` environment variable for use with the nodectl application:

```bash
export NODECTL_TOKEN="<token-from-step-10>"
```

For example:

```bash
export NODECTL_TOKEN="hvs.CAESIGxyz..."
```

### Persist Environment Variables

To persist these variables, add them to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.):

```bash
# HCP Vault Configuration for nodectl
export VAULT_ADDR="https://your-cluster.hashicorp.cloud:8200"
export VAULT_NAMESPACE="admin/nodectl"
export NODECTL_TOKEN="hvs.CAESIGxyz..."
```

---

## Quick Reference Commands

```bash
# Set up environment
export VAULT_ADDR="<Your-Public-Cluster-URL>"
export VAULT_TOKEN="<Your-Admin-Token>"
export VAULT_NAMESPACE="admin"

# Create namespace
vault namespace create nodectl

# Switch to nodectl namespace
export VAULT_NAMESPACE="admin/nodectl"

# Enable transit engine
vault secrets enable transit

# Create policy
vault policy write nodectl-service-policy nodectl-policy.hcl

# Enable approle
vault auth enable approle

# Create role
vault write auth/approle/role/nodectl-app token_policies="nodectl-service-policy" token_ttl=1h token_max_ttl=4h

# Get credentials and login
ROLE_ID=$(vault read -field=role_id auth/approle/role/nodectl-app/role-id)
SECRET_ID=$(vault write -force -field=secret_id auth/approle/role/nodectl-app/secret-id)
export NODECTL_TOKEN=$(vault write -field=token auth/approle/login role_id="$ROLE_ID" secret_id="$SECRET_ID")
```

---

## Troubleshooting

### Error: "http: server gave HTTP response to HTTPS client"

This occurs when `VAULT_ADDR` is not set or is using the wrong protocol. Ensure you set:

```bash
export VAULT_ADDR="https://your-cluster.hashicorp.cloud:8200"
```

### Error: "permission denied"

Ensure you're using the correct token with appropriate permissions and the correct namespace:

```bash
export VAULT_NAMESPACE="admin/nodectl"
export VAULT_TOKEN="<your-admin-token>"
```

### Token Expired

AppRole tokens expire based on the TTL settings. Generate a new token:

```bash
export NODECTL_TOKEN=$(vault write -field=token auth/approle/login role_id="$ROLE_ID" secret_id="$SECRET_ID")
```

---

## References

- [nodectl Setup Guide](./nodectl-setup.md) — main setup instructions for nodectl
- [HCP Vault Dedicated - Create a Cluster](https://developer.hashicorp.com/vault/tutorials/get-started-hcp-vault-dedicated/vault-create-cluster)
- [Manage Access to Secrets](https://developer.hashicorp.com/vault/tutorials/get-started-hcp-vault-dedicated/manage-access-secrets)
- [Authenticate Users](https://developer.hashicorp.com/vault/tutorials/get-started-hcp-vault-dedicated/vault-auth-method)
- [Transit Secrets Engine](https://developer.hashicorp.com/vault/docs/secrets/transit)
- [AppRole Auth Method](https://developer.hashicorp.com/vault/docs/auth/approle)
