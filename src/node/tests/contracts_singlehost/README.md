# How to use

## Prerequisites
- Node.js installed
- TypeScript installed globally (`npm install -g typescript`)
- Blueprint installed (`npm install -g blueprint`)

## Counter contract

### Build contract

`npx blueprint build Counter`

### Deploy to singlehost network

`npx tsx scripts/deployCounterSingleHost.ts`

### Call contract

`npx tsx scripts/callCounterState.ts`
`npx tsx scripts/getCounterState.ts`

## Ping Pong contract

### Build contract

`npx blueprint build PingPong`

### Deploy to singlehost network

The script deploys a number of ping pong pairs and starts it all.
`npx tsx scripts/deployPingPongSingleHost.ts`

### Call contract

The script get state of each ping pong contract and prints it. In case of error it will printed.
When ping pong ends all the accounts must have 64 "1" bits in their accumulator.
It needs a lot of time to reach this state.
`npx tsx scripts/getPingPongState.ts`
