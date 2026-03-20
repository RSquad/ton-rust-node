import { NetworkProvider } from "@ton/blueprint";
import { Address, Cell, Tuple, TupleBuilder, TupleItemInt, TupleReader } from "@ton/core";

export async function run(provider: NetworkProvider) {
    const configAddress = Address.parse("-1:5555555555555555555555555555555555555555555555555555555555555555");
    const contract = provider.provider(configAddress);
    const result = await contract.get("list_proposals", []);
    const list = result.stack.pop();
    if (list.type === 'null') {
        console.log(`no active proposals`);
        return;
    }
    if (list.type !== 'tuple') {
        throw new Error(`list is not a tuple`);
    }
    for (const proposal of (list as Tuple).items) {
        const items = proposal as unknown as any[];
        const phash = items[0] as bigint;
        const fields = items[1] as any[];
        const expires = fields[0] as bigint;
        const critical = fields[1] as bigint;
        const params = fields[2] as any[];
        const [id, val, hash] = [params[0] as bigint, params[1], params[2] as bigint];
        const vset_id = fields[3] as bigint;
        const voters = fields[4] as bigint[];
        const weightRemaining = fields[5] as bigint;
        const roundsRemaining = fields[6] as bigint;
        const losses = fields[7] as bigint; 
        const wins = fields[8] as bigint;
        
        console.log(`proposal: `);
        console.log(`  proposal hash: ${phash.toString(16).padStart(64, '0')}`);
        console.log(`  expires: ${expires}`);
        console.log(`  critical: ${critical}`);
        console.log(`  id: ${id}`);
        //console.log(`  val: ${val}`);
        console.log(`  required current config hash: ${hash.toString(16).padStart(64, '0')}`);
        console.log(`  vset_id: ${vset_id.toString(16).padStart(64, '0')}`);
        console.log(`  weightRemaining: ${weightRemaining}`);
        console.log(`  roundsRemaining: ${roundsRemaining}`);
        console.log(`  losses: ${losses}`);
        console.log(`  wins: ${wins}`);

        console.log(`  voters:`);
        console.log(`${voters.join(', ')}`);
    }
}
