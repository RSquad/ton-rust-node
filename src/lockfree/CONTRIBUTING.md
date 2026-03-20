Don't be afraid. If you want to discuss about the design of any structure, feel
free to open an issue. If you have any doubt on how any structure works, you
can open an issue as well. I am open to discuss anything :D

But BEWARE! You will face the dark arts of unsafe Rust!

# Documentation
If you write any method, iterator, data structure or whatever, you are not
required to write any documentation. However, writing documentation helps me
(or any other person) to understand what is its purpouse. But don't worry,
if you did not wrote any documentation and I need more info, I am going to
ask for the info.

# Testing
Besides writing unit tests, if you wrote anything sensitive or with `unsafe`
keyword, I would like you to write a new fuzz test. To add a new fuzz test,
just execute `cargo fuzz add <your_test_name>`, and to run
`./run-fuzz.sh <your_test_name> --sanitizer <choose-a-sanitizer>`. Be sure
to have installed `cargo-fuzz`. Write the test using the tiny "framework" I
wrote, named `fuzzsuite`, available as local package, in `fuzz/fuzzsuite.
You can look at other fuzz tests as examples. To pass a flag to libfuzzer, use
the environmental variable LFUZ_OPTIONS.
