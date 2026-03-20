# CPU info
```
Architecture:        x86_64
CPU op-mode(s):      32-bit, 64-bit
Byte Order:          Little Endian
Address sizes:       39 bits physical, 48 bits virtual
CPU(s):              4
On-line CPU(s) list: 0-3
Thread(s) per core:  2
Core(s) per socket:  2
Socket(s):           1
NUMA node(s):        1
Vendor ID:           GenuineIntel
CPU family:          6
Model:               142
Model name:          Intel(R) Core(TM) i5-7200U CPU @ 2.50GHz
Stepping:            9
CPU MHz:             2574.592
CPU max MHz:         3100.0000
CPU min MHz:         400.0000
BogoMIPS:            5426.00
Virtualization:      VT-x
L1d cache:           32K
L1i cache:           32K
L2 cache:            256K
L3 cache:            3072K
NUMA node0 CPU(s):   0-3
Flags:               fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush dts acpi mmx fxsr sse sse2 ss ht tm pbe syscall nx pdpe1gb rdtscp lm constant_tsc art arch_perfmon pebs bts rep_good nopl xtopology nonstop_tsc cpuid aperfmperf tsc_known_freq pni pclmulqdq dtes64 monitor ds_cpl vmx est tm2 ssse3 sdbg fma cx16 xtpr pdcm pcid sse4_1 sse4_2 x2apic movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand lahf_lm abm 3dnowprefetch cpuid_fault epb invpcid_single pti ssbd ibrs ibpb stibp tpr_shadow vnmi flexpriority ept vpid ept_ad fsgsbase tsc_adjust bmi1 avx2 smep bmi2 erms invpcid mpx rdseed adx smap clflushopt intel_pt xsaveopt xsavec xgetbv1 xsaves dtherm ida arat pln pts hwp hwp_notify hwp_act_window hwp_epp flush_l1d
```
Running on Linux 4.19.1-arch1-1-ARCH #1 SMP PREEMPT Sun Nov 4 16:49:26 UTC 2018 x86_64 GNU/Linux

# Benchmarks
Benchmark code under [benchmark](benchmark) directory.
More rounds per seconds is better.

As you can see, there is a lot to improve!


## THREAD-LOCAL STORAGE
```
Result for 1 threads:
Target 0 (std/global):
mean of 563244497.238 r/s (704108544 rounds in 1.250 seconds)
Target 1 (blocking):
mean of 127740592.346 r/s (159698944 rounds in 1.250 seconds)
Target 2 (blocking with cached access):
mean of 322415766.353 r/s (403059712 rounds in 1.250 seconds)
Target 3 (lockfree):
mean of 139950785.210 r/s (174964736 rounds in 1.250 seconds)
Target 4 (lockfree with cached id):
mean of 204579727.638 r/s (255761408 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (std/global):
mean of 1254346568.165 r/s (1568060416 rounds in 1.250 seconds)
Target 1 (blocking):
mean of 201892643.702 r/s (252385280 rounds in 1.250 seconds)
Target 2 (blocking with cached access):
mean of 341823902.079 r/s (427312128 rounds in 1.250 seconds)
Target 3 (lockfree):
mean of 279972809.868 r/s (349997056 rounds in 1.250 seconds)
Target 4 (lockfree with cached id):
mean of 407243227.338 r/s (509092864 rounds in 1.250 seconds)

Result for 16 threads:
Target 0 (std/global):
mean of 1265788513.195 r/s (1582526464 rounds in 1.250 seconds)
Target 1 (blocking):
mean of 146437704.110 r/s (183084032 rounds in 1.250 seconds)
Target 2 (blocking with cached access):
mean of 187998457.204 r/s (235046912 rounds in 1.250 seconds)
Target 3 (lockfree):
mean of 280079755.878 r/s (350157824 rounds in 1.250 seconds)
Target 4 (lockfree with cached id):
mean of 410127128.874 r/s (512739328 rounds in 1.250 seconds)

Result for 32 threads:
Target 0 (std/global):
mean of 1253856389.371 r/s (1567800320 rounds in 1.250 seconds)
Target 1 (blocking):
mean of 117252735.879 r/s (146627584 rounds in 1.251 seconds)
Target 2 (blocking with cached access):
mean of 146574797.405 r/s (183299072 rounds in 1.251 seconds)
Target 3 (lockfree):
mean of 279772786.027 r/s (349842432 rounds in 1.250 seconds)
Target 4 (lockfree with cached id):
mean of 410133726.205 r/s (512842752 rounds in 1.250 seconds)

Result for 128 threads:
Target 0 (std/global):
mean of 1253819121.657 r/s (1569345536 rounds in 1.252 seconds)
Target 1 (blocking):
mean of 56362911.831 r/s (70654976 rounds in 1.254 seconds)
Target 2 (blocking with cached access):
mean of 61073798.426 r/s (76540928 rounds in 1.253 seconds)
Target 3 (lockfree):
mean of 279925671.251 r/s (350430208 rounds in 1.252 seconds)
Target 4 (lockfree with cached id):
mean of 409816050.587 r/s (512987136 rounds in 1.252 seconds)

```

## QUEUE
```
Result for 1 threads:
Target 0 (mutex vector):
mean of 46009691.533 r/s (57522176 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 29984570.170 r/s (37487616 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 14504967.628 r/s (18134016 rounds in 1.250 seconds)

Result for 2 threads:
Target 0 (mutex vector):
mean of 11336923.499 r/s (14173184 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 3333379.883 r/s (4167680 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 5218179.185 r/s (6523904 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (mutex vector):
mean of 8587407.595 r/s (10736640 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 5018169.267 r/s (6275072 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 6167520.930 r/s (7711744 rounds in 1.250 seconds)

Result for 8 threads:
Target 0 (mutex vector):
mean of 8277195.290 r/s (10351616 rounds in 1.251 seconds)
Target 1 (mutex linked list):
mean of 4920637.475 r/s (6155264 rounds in 1.251 seconds)
Target 2 (lockfree):
mean of 5725833.254 r/s (7340032 rounds in 1.282 seconds)

Result for 16 threads:
Target 0 (mutex vector):
mean of 8258765.613 r/s (10331136 rounds in 1.251 seconds)
Target 1 (mutex linked list):
mean of 4928023.480 r/s (6169600 rounds in 1.252 seconds)
Target 2 (lockfree):
mean of 5764235.033 r/s (7392256 rounds in 1.282 seconds)

```

## STACK
```

Result for 1 threads:
Target 0 (mutex vector):
mean of 48626690.729 r/s (60792832 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 29654579.252 r/s (37074944 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 19572942.644 r/s (24470528 rounds in 1.250 seconds)

Result for 2 threads:
Target 0 (mutex vector):
mean of 8048881.361 r/s (10061824 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 4044680.737 r/s (5056512 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 5921813.570 r/s (7403520 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (mutex vector):
mean of 7946160.211 r/s (9934848 rounds in 1.250 seconds)
Target 1 (mutex linked list):
mean of 4890576.599 r/s (6115328 rounds in 1.250 seconds)
Target 2 (lockfree):
mean of 6409920.374 r/s (8014848 rounds in 1.250 seconds)

Result for 8 threads:
Target 0 (mutex vector):
mean of 7650499.209 r/s (9567232 rounds in 1.251 seconds)
Target 1 (mutex linked list):
mean of 4813496.739 r/s (6020096 rounds in 1.251 seconds)
Target 2 (lockfree):
mean of 6670965.273 r/s (8513536 rounds in 1.276 seconds)

Result for 16 threads:
Target 0 (mutex vector):
mean of 7615925.275 r/s (9525248 rounds in 1.251 seconds)
Target 1 (mutex linked list):
mean of 4796021.841 r/s (6003712 rounds in 1.252 seconds)
Target 2 (lockfree):
mean of 6442273.200 r/s (8230912 rounds in 1.278 seconds)

```

## MAP
```
Result for 1 threads:
Target 0 (mutex insert):
mean of 3266460.771 r/s (4520960 rounds in 1.384 seconds)
Target 1 (lockfree insert):
mean of 2006847.396 r/s (2509824 rounds in 1.251 seconds)

Result for 2 threads:
Target 0 (mutex insert):
mean of 1998542.799 r/s (2498560 rounds in 1.250 seconds)
Target 1 (lockfree insert):
mean of 2494350.353 r/s (3119104 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (mutex insert):
mean of 2295868.648 r/s (2873344 rounds in 1.252 seconds)
Target 1 (lockfree insert):
mean of 3688243.156 r/s (4612096 rounds in 1.250 seconds)

Result for 8 threads:
Target 0 (mutex insert):
mean of 2505145.993 r/s (3135488 rounds in 1.252 seconds)
Target 1 (lockfree insert):
mean of 3923203.772 r/s (4945920 rounds in 1.261 seconds)

Result for 1 threads:
Target 0 (mutex get):
mean of 6535583.089 r/s (8170496 rounds in 1.250 seconds)
Target 1 (lockfree get):
mean of 4508820.420 r/s (5637120 rounds in 1.250 seconds)

Result for 2 threads:
Target 0 (mutex get):
mean of 2058094.237 r/s (2573312 rounds in 1.250 seconds)
Target 1 (lockfree get):
mean of 7697488.160 r/s (9623552 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (mutex get):
mean of 2417756.745 r/s (3023872 rounds in 1.251 seconds)
Target 1 (lockfree get):
mean of 10679469.481 r/s (13352960 rounds in 1.250 seconds)

Result for 8 threads:
Target 0 (mutex get):
mean of 2577102.331 r/s (3224576 rounds in 1.251 seconds)
Target 1 (lockfree get):
mean of 10314698.163 r/s (12916736 rounds in 1.252 seconds)

Result for 1 threads:
Target 0 (mutex remove):
mean of 21127356.471 r/s (26413056 rounds in 1.250 seconds)
Target 1 (lockfree remove):
mean of 3093448.918 r/s (3867648 rounds in 1.250 seconds)

Result for 2 threads:
Target 0 (mutex remove):
mean of 12228997.875 r/s (15288320 rounds in 1.250 seconds)
Target 1 (lockfree remove):
mean of 9171024.742 r/s (11464704 rounds in 1.250 seconds)

Result for 4 threads:
Target 0 (mutex remove):
mean of 10815691.241 r/s (13521920 rounds in 1.250 seconds)
Target 1 (lockfree remove):
mean of 11237987.111 r/s (14049280 rounds in 1.250 seconds)

Result for 8 threads:
Target 0 (mutex remove):
mean of 10836043.306 r/s (13548544 rounds in 1.250 seconds)
Target 1 (lockfree remove):
mean of 11590965.619 r/s (14494720 rounds in 1.251 seconds)

Result for 1 threads:
Target 0 (mutex mixed):
mean of 4552824.329 r/s (5692416 rounds in 1.250 seconds)
Target 1 (lockfree mixed):
mean of 2001439.755 r/s (2502656 rounds in 1.250 seconds)

Result for 2 threads:
Target 0 (mutex mixed):
mean of 1520136.949 r/s (1901568 rounds in 1.251 seconds)
Target 1 (lockfree mixed):
mean of 1099053.074 r/s (1376256 rounds in 1.252 seconds)

Result for 4 threads:
Target 0 (mutex mixed):
mean of 1297798.011 r/s (1624064 rounds in 1.251 seconds)
Target 1 (lockfree mixed):
mean of 1868782.116 r/s (2338816 rounds in 1.252 seconds)

Result for 8 threads:
Target 0 (mutex mixed):
mean of 1422417.889 r/s (1782784 rounds in 1.253 seconds)
Target 1 (lockfree mixed):
mean of 1966307.117 r/s (2547712 rounds in 1.296 seconds)

```

## MPSC CHANNEL
```
Mutexed VecDeque with 3 threads total time: 195.828541ms
Std's MPSC with 3 threads total time: 144.951974ms
Lockfree MPSC with 3 threads total time: 112.348453ms

Mutexed VecDeque with 5 threads total time: 422.126165ms
Std's MPSC with 5 threads total time: 250.879545ms
Lockfree MPSC with 5 threads total time: 201.960734ms

Mutexed VecDeque with 9 threads total time: 803.066437ms
Std's MPSC with 9 threads total time: 479.966615ms
Lockfree MPSC with 9 threads total time: 385.762984ms

Mutexed VecDeque with 17 threads total time: 1.584427583s
Std's MPSC with 17 threads total time: 978.197558ms
Lockfree MPSC with 17 threads total time: 744.057727ms

Mutexed VecDeque with 33 threads total time: 3.086542744s
Std's MPSC with 33 threads total time: 1.973712673s
Lockfree MPSC with 33 threads total time: 1.544679447s
```

## SPSC CHANNEL
```
Mutexed VecDeque total time: 759.772544ms
Std's MPSC (as SPSC) total time: 96.751346ms
Lockfree SPSC total time: 384.928062ms
```

## SPMC CHANNEL
```
Mutexed VecDeque with 3 threads total time: 227.881661ms
Mutexed Std's MPSC (as SPMC) with 3 threads total time: 304.818219ms
Lockfree SPMC with 3 threads total time: 111.830327ms

Mutexed VecDeque with 5 threads total time: 363.383696ms
Mutexed Std's MPSC (as SPMC) with 5 threads total time: 158.106779ms
Lockfree SPMC with 5 threads total time: 88.795666ms

Mutexed VecDeque with 9 threads total time: 722.081453ms
Mutexed Std's MPSC (as SPMC) with 9 threads total time: 166.285144ms
Lockfree SPMC with 9 threads total time: 90.274454ms

Mutexed VecDeque with 17 threads total time: 1.168055717s
Mutexed Std's MPSC (as SPMC) with 17 threads total time: 164.420737ms
Lockfree SPMC with 17 threads total time: 169.7209ms

Mutexed VecDeque with 33 threads total time: 2.262141374s
Mutexed Std's MPSC (as SPMC) with 33 threads total time: 240.266825ms
Lockfree SPMC with 33 threads total time: 555.498619ms
```

## MPMC CHANNEL
```
Mutexed VecDeque with 4 threads total time: 85.693921ms
Mutexed Std's MPSC (as MPMC)  with 4 threads total time: 147.843224ms
Lockfree MPMC with 4 threads total time: 47.507121ms

Mutexed VecDeque with 8 threads total time: 181.006411ms
Mutexed Std's MPSC (as MPMC)  with 8 threads total time: 178.166068ms
Lockfree MPMC with 8 threads total time: 92.244031ms

Mutexed VecDeque with 16 threads total time: 392.331879ms
Mutexed Std's MPSC (as MPMC)  with 16 threads total time: 403.369533ms
Lockfree MPMC with 16 threads total time: 324.461572ms
```

## REQUEST PROGRAM
```
A program simulating a concurrent server.

Mutexed HashMap and Std's MPSC with 2 threads total time: 793.554071ms
Lockfree structures with 2 threads total time: 490.944222ms

Mutexed HashMap and Std's MPSC with 4 threads total time: 369.968509ms
Lockfree structures with 4 threads total time: 409.906746ms

Mutexed HashMap and Std's MPSC with 8 threads total time: 390.376975ms
Lockfree structures with 8 threads total time: 412.722696ms

Mutexed HashMap and Std's MPSC with 16 threads total time: 386.407811ms
Lockfree structures with 16 threads total time: 325.562949ms
```

## MESSAGE REVERB PROGRAM
```
A program which reverberates messages through a plain queue channel

Mutexed VecDeque with 2 threads total time: 220.035769ms
Mutexed LinkedList with 2 threads total time: 586.576805ms
Lockfree Queue with 2 threads total time: 197.928229ms

Mutexed VecDeque with 4 threads total time: 238.337063ms
Mutexed LinkedList with 4 threads total time: 453.672894ms
Lockfree Queue with 4 threads total time: 158.472864ms

Mutexed VecDeque with 8 threads total time: 243.002558ms
Mutexed LinkedList with 8 threads total time: 477.545286ms
Lockfree Queue with 8 threads total time: 155.281614ms

Mutexed VecDeque with 16 threads total time: 238.828929ms
Mutexed LinkedList with 16 threads total time: 476.347392ms
Lockfree Queue with 16 threads total time: 141.994415ms
```

## HASH MINING
```
A program simulating a hash miner.

Mutexed structures with 2 threads total time: 454.91082ms
Lockfree structures with 2 threads total time: 481.382395ms

Mutexed structures with 4 threads total time: 317.253501ms
Lockfree structures with 4 threads total time: 323.152586ms

Mutexed structures with 8 threads total time: 250.116503ms
Lockfree structures with 8 threads total time: 260.129446ms

Mutexed structures with 16 threads total time: 245.25719ms
Lockfree structures with 16 threads total time: 246.603846ms

Mutexed structures with 32 threads total time: 247.683849ms
Lockfree structures with 32 threads total time: 245.1651ms

Mutexed structures with 64 threads total time: 249.568002ms
Lockfree structures with 64 threads total time: 261.512353ms

Mutexed structures with 128 threads total time: 327.423669ms
Lockfree structures with 128 threads total time: 336.425139ms
```

