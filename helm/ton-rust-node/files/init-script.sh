set -eu
MAIN=/main
STATIC=$MAIN/static
SEED=/seed

mkdir -p "$STATIC"

cp "$SEED/global-config/global.config.json" "$MAIN/global.config.json"
cp "$SEED/logs-config/logs.config.yml" "$MAIN/logs.config.yml"

for f in "$SEED/zerostate/zerostate.boc" "$SEED/basestate/basestate.boc"; do
  [ -f "$f" ] || continue
  hash=$(sha256sum "$f" | cut -d' ' -f1)
  cp "$f" "$STATIC/$hash.boc"
done

POD_INDEX=${POD_NAME##*-}
cp "$SEED/node-configs/node-$POD_INDEX.json" "$MAIN/config.json"

chown -R 1000:1000 "$MAIN"
