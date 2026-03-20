import { TonClient } from "@ton/ton";

const tonClient = new TonClient({ endpoint: process.env.API_ENDPOINT + "/jsonRPC", timeout: 20, });