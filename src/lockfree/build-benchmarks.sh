#!/usr/bin/env sh

cd benchmark
FILE=../BENCHMARKS.md

truncate --size 0 $FILE

echo '# CPU info' >> $FILE
echo '```' >> $FILE
lscpu >> $FILE
echo '```' >> $FILE
echo 'Running on' $(uname -s -r -v -m -o) >> $FILE
echo '' >> $FILE

echo '# Benchmarks' >> $FILE
echo 'Benchmark code under [benchmark](benchmark) directory.' >> $FILE
echo 'More rounds per seconds is better.' >> $FILE
echo '' >> $FILE
echo 'As you can see, there is a lot to improve!' >> $FILE
echo '' >> $FILE
echo '' >> $FILE

echo '## THREAD-LOCAL STORAGE' >> $FILE
echo '```' >> $FILE
cargo run --bin tls --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## QUEUE' >> $FILE
echo '```' >> $FILE
cargo run --bin queue --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## STACK' >> $FILE
echo '```' >> $FILE
echo '' >> $FILE
cargo run --bin stack --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## MAP' >> $FILE
echo '```' >> $FILE
cargo run --bin map --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## MPSC CHANNEL' >> $FILE
echo '```' >> $FILE
cargo run --bin mpsc --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## SPSC CHANNEL' >> $FILE
echo '```' >> $FILE
cargo run --bin spsc --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## SPMC CHANNEL' >> $FILE
echo '```' >> $FILE
cargo run --bin spmc --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## MPMC CHANNEL' >> $FILE
echo '```' >> $FILE
cargo run --bin mpmc --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## REQUEST PROGRAM' >> $FILE
echo '```' >> $FILE
cargo run --bin request --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## MESSAGE REVERB PROGRAM' >> $FILE
echo '```' >> $FILE
cargo run --bin reverb --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE

echo '## HASH MINING' >> $FILE
echo '```' >> $FILE
cargo run --bin mining --release >> $FILE || exit 1
echo '```' >> $FILE
echo '' >> $FILE
