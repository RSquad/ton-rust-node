import vault from "node-vault";

export async function exportPublicKeyFromVault(secretId: string): Promise<Buffer> {
    //const vaultAddr = process.env.VAULT_ADDR || 'http://localhost:8200';
    //const vaultToken = process.env.VAULT_TOKEN;
    //const vaultNamespace = process.env.VAULT_NAMESPACE;
    const vaultClient = vault({ 
        apiVersion: 'v1', 
        //endpoint: vaultAddr, 
        //namespace: vaultNamespace, 
        //token: vaultToken,
        requestOptions: { strictSSL: false }
    });

    const keyData = await vaultClient.read(`transit/keys/${secretId}`);
    const publicKey = keyData.data.keys["1"].public_key;
    return Buffer.from(publicKey, "base64");
}