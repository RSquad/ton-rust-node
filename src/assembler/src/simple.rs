/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    errors::ToOperationParameterError, parse::*, CompileHandler, CompileResult, Engine,
    EnsureParametersCountInRange, Units,
};
use crate::{
    debug::{DbgNode, DbgPos},
    simple_commands,
};

// Compilation engine *********************************************************

impl Engine {
    #[rustfmt::skip]
    simple_commands! {
        enumerate_simple_commands
        ABS                                  => 0xB6, 0x0B
        ACCEPT                               => 0xF8, 0x00
        ADD                                  => 0xA0
        ADDCONST z = parse_const_i8          => 0xA6, z
        ADDRAND                              => 0xF8, 0x15
        AGAIN                                => 0xEA
        AGAINBRK                             => 0xE3, 0x1A
        AGAINEND                             => 0xEB
        AGAINENDBRK                          => 0xE3, 0x1B
        AND                                  => 0xB0
        ATEXIT                               => 0xED, 0xF3
        ATEXITALT                            => 0xED, 0xF4
        BALANCE                              => 0xF8, 0x27
        BBITREFS                             => 0xCF, 0x33
        BBITS                                => 0xCF, 0x31
        BCHKBITREFS                          => 0xCF, 0x3B
        BCHKBITREFSQ                         => 0xCF, 0x3F
        BCHKREFS                             => 0xCF, 0x3A
        BCHKREFSQ                            => 0xCF, 0x3E
        BDEPTH                               => 0xCF, 0x30
        BINDUMP                              => 0xFE, 0x12
        BINPRINT                             => 0xFE, 0x13
        BITSIZE                              => 0xB6, 0x02
        BLESS                                => 0xED, 0x1E
        BLESSARGS c1 = parse_const_u4;
                  c2 = parse_const_i4        => 0xEE, (c1 << 4) | c2
        BLESSNUMARGS c = parse_const_u4_14   => 0xEE, c
        BLESSVARARGS                         => 0xED, 0x1F
        BLKDROP c = parse_const_u4           => 0x5F, c
        BLKDROP2 c1 = parse_const_u4_nonzero;
                 c2 = parse_const_u4         => 0x6C, (c1 << 4) | c2
        BLKPUSH c1 = parse_const_u4_nonzero;
                c2 = parse_const_u4          => 0x5F, (c1 << 4) | c2
        BLKSWAP c1 = parse_const_u4_plus_one;
                c2 = parse_const_u4_plus_one => 0x55, (c1 << 4) | c2
        BLKSWX                               => 0x63
        BLOCKLT                              => 0xF8, 0x24
        BLS_VERIFY                           => 0xF9, 0x30, 0x00
        BLS_AGGREGATE                        => 0xF9, 0x30, 0x01
        BLS_FASTAGGREGATEVERIFY              => 0xF9, 0x30, 0x02
        BLS_AGGREGATEVERIFY                  => 0xF9, 0x30, 0x03
        BLS_G1_ADD                           => 0xF9, 0x30, 0x10
        BLS_G1_SUB                           => 0xF9, 0x30, 0x11
        BLS_G1_NEG                           => 0xF9, 0x30, 0x12
        BLS_G1_MUL                           => 0xF9, 0x30, 0x13
        BLS_G1_MULTIEXP                      => 0xF9, 0x30, 0x14
        BLS_G1_ZERO                          => 0xF9, 0x30, 0x15
        BLS_MAP_TO_G1                        => 0xF9, 0x30, 0x16
        BLS_G1_INGROUP                       => 0xF9, 0x30, 0x17
        BLS_G1_ISZERO                        => 0xF9, 0x30, 0x18
        BLS_G2_ADD                           => 0xF9, 0x30, 0x20
        BLS_G2_SUB                           => 0xF9, 0x30, 0x21
        BLS_G2_NEG                           => 0xF9, 0x30, 0x22
        BLS_G2_MUL                           => 0xF9, 0x30, 0x23
        BLS_G2_MULTIEXP                      => 0xF9, 0x30, 0x24
        BLS_G2_ZERO                          => 0xF9, 0x30, 0x25
        BLS_MAP_TO_G2                        => 0xF9, 0x30, 0x26
        BLS_G2_INGROUP                       => 0xF9, 0x30, 0x27
        BLS_G2_ISZERO                        => 0xF9, 0x30, 0x28
        BLS_PAIRING                          => 0xF9, 0x30, 0x30
        BLS_PUSHR                            => 0xF9, 0x30, 0x31
        BOOLAND                              => 0xED, 0xF0
        BOOLEVAL                             => 0xED, 0xF9
        BOOLOR                               => 0xED, 0xF1
        BRANCH                               => 0xDB, 0x32
        BREFS                                => 0xCF, 0x32
        BREMBITS                             => 0xCF, 0x35
        BREMBITREFS                          => 0xCF, 0x37
        BREMREFS                             => 0xCF, 0x36
        BTOS                                 => 0xCF, 0x50
        BUYGAS                               => 0xF8, 0x02
        CADR                                 => 0x6F, 0xB4
        CADDR                                => 0x6F, 0xD4
        CDDR                                 => 0x6F, 0xB5
        CDDDR                                => 0x6F, 0xD5
        CALLCC                               => 0xDB, 0x34
        CALLCCARGS  c1 = parse_const_u4;
                    c2 = parse_const_i4      => 0xDB, 0x36, (c1 << 4) | c2
        CALLCCVARARGS                        => 0xDB, 0x3B
        CALLX                                => 0xD8
        CALLXVARARGS                         => 0xDB, 0x38
        CAR                                  => 0x6F, 0x10
        CDR                                  => 0x6F, 0x11
        CDATASIZE                            => 0xF9, 0x41
        CDATASIZEQ                           => 0xF9, 0x40
        CDEPTH                               => 0xD7, 0x65
        CDEPTHI c = parse_const_u2           => 0xD7, 0x6c + c
        CDEPTHIX                             => 0xD7, 0x71
        CHANGELIB                            => 0xFB, 0x07
        CHASHI  c = parse_const_u2           => 0xD7, 0x68 + c
        CHASHIX                              => 0xD7, 0x70
        CHKBOOL                              => 0xB4, 0x00
        CHKBIT                               => 0xB5, 0x00
        CHKNAN                               => 0xC5
        CHKDEPTH                             => 0x69
        CHKSIGNS                             => 0xF9, 0x11
        CHKSIGNU                             => 0xF9, 0x10
        CHKTUPLE                             => 0x6F, 0x30
        CLEVEL                               => 0xD7, 0x66
        CLEVELMASK                           => 0xD7, 0x67
        CMP                                  => 0xBF
        COMMA                                => 0x6F, 0x8C
        COMMIT                               => 0xF8, 0x0F
        COMPOS                               => 0xED, 0xF0
        COMPOSALT                            => 0xED, 0xF1
        COMPOSBOTH                           => 0xED, 0xF2
        CONDSEL                              => 0xE3, 0x04
        CONDSELCHK                           => 0xE3, 0x05
        CONFIGROOT                           => 0xF8, 0x29
        CONFIGDICT                           => 0xF8, 0x30
        CONFIGPARAM                          => 0xF8, 0x32
        CONFIGOPTPARAM                       => 0xF8, 0x33
        CONS                                 => 0x6F, 0x02
        CTOS                                 => 0xD0
        DEC                                  => 0xA5
        DEBUG z = parse_const_u8_240         => 0xFE, z
        DEBUGOFF                             => 0xFE, 0x1E
        DEBUGON                              => 0xFE, 0x1F
        DEPTH                                => 0x68
        DICTADD                              => 0xF4, 0x32
        DICTADDB                             => 0xF4, 0x51
        DICTADDGET                           => 0xF4, 0x3A
        DICTADDGETB                          => 0xF4, 0x55
        DICTADDGETREF                        => 0xF4, 0x3B
        DICTADDREF                           => 0xF4, 0x33
        DICTDEL                              => 0xF4, 0x59
        DICTDELGET                           => 0xF4, 0x62
        DICTDELGETREF                        => 0xF4, 0x63
        DICTEMPTY                            => 0x6E
        DICTGET                              => 0xF4, 0x0A
        DICTGETNEXT                          => 0xF4, 0x74
        DICTGETNEXTEQ                        => 0xF4, 0x75
        DICTGETOPTREF                        => 0xF4, 0x69
        DICTIGETOPTREF                       => 0xF4, 0x6A
        DICTUGETOPTREF                       => 0xF4, 0x6B
        DICTSETGETOPTREF                     => 0xF4, 0x6D
        DICTISETGETOPTREF                    => 0xF4, 0x6E
        DICTUSETGETOPTREF                    => 0xF4, 0x6F
        DICTGETPREV                          => 0xF4, 0x76
        DICTGETPREVEQ                        => 0xF4, 0x77
        DICTGETREF                           => 0xF4, 0x0B
        DICTIADD                             => 0xF4, 0x34
        DICTIADDB                            => 0xF4, 0x52
        DICTIADDGET                          => 0xF4, 0x3C
        DICTIADDGETB                         => 0xF4, 0x56
        DICTIADDGETREF                       => 0xF4, 0x3D
        DICTIADDREF                          => 0xF4, 0x35
        DICTIDEL                             => 0xF4, 0x5A
        DICTIDELGET                          => 0xF4, 0x64
        DICTIDELGETREF                       => 0xF4, 0x65
        DICTIGET                             => 0xF4, 0x0C
        DICTIGETEXEC                         => 0xF4, 0xA2
        DICTIGETEXECZ                        => 0xF4, 0xBE
        DICTIGETJMP                          => 0xF4, 0xA0
        DICTIGETJMPZ                         => 0xF4, 0xBC
        DICTIGETNEXT                         => 0xF4, 0x78
        DICTIGETNEXTEQ                       => 0xF4, 0x79
        DICTIGETPREV                         => 0xF4, 0x7A
        DICTIGETPREVEQ                       => 0xF4, 0x7B
        DICTIGETREF                          => 0xF4, 0x0D
        DICTIMAX                             => 0xF4, 0x8C
        DICTIMAXREF                          => 0xF4, 0x8D
        DICTIMIN                             => 0xF4, 0x84
        DICTIMINREF                          => 0xF4, 0x85
        DICTIREMMAX                          => 0xF4, 0x9C
        DICTIREMMAXREF                       => 0xF4, 0x9D
        DICTIREMMIN                          => 0xF4, 0x94
        DICTIREMMINREF                       => 0xF4, 0x95
        DICTIREPLACE                         => 0xF4, 0x24
        DICTIREPLACEB                        => 0xF4, 0x4A
        DICTIREPLACEGET                      => 0xF4, 0x2C
        DICTIREPLACEGETB                     => 0xF4, 0x4E
        DICTIREPLACEGETREF                   => 0xF4, 0x2D
        DICTIREPLACEREF                      => 0xF4, 0x25
        DICTISET                             => 0xF4, 0x14
        DICTISETB                            => 0xF4, 0x42
        DICTISETGET                          => 0xF4, 0x1C
        DICTISETGETB                         => 0xF4, 0x46
        DICTISETGETREF                       => 0xF4, 0x1D
        DICTISETREF                          => 0xF4, 0x15
        DICTMAX                              => 0xF4, 0x8A
        DICTMAXREF                           => 0xF4, 0x8B
        DICTMIN                              => 0xF4, 0x82
        DICTMINREF                           => 0xF4, 0x83
        DICTREMMAX                           => 0xF4, 0x9A
        DICTREMMAXREF                        => 0xF4, 0x9B
        DICTREMMIN                           => 0xF4, 0x92
        DICTREMMINREF                        => 0xF4, 0x93
        DICTREPLACE                          => 0xF4, 0x22
        DICTREPLACEB                         => 0xF4, 0x49
        DICTREPLACEGET                       => 0xF4, 0x2A
        DICTREPLACEGETB                      => 0xF4, 0x4D
        DICTREPLACEGETREF                    => 0xF4, 0x2B
        DICTREPLACEREF                       => 0xF4, 0x23
        DICTSET                              => 0xF4, 0x12
        DICTSETB                             => 0xF4, 0x41
        DICTSETGET                           => 0xF4, 0x1A
        DICTSETGETB                          => 0xF4, 0x45
        DICTSETGETREF                        => 0xF4, 0x1B
        DICTSETREF                           => 0xF4, 0x13
        DICTUADD                             => 0xF4, 0x36
        DICTUADDB                            => 0xF4, 0x53
        DICTUADDGET                          => 0xF4, 0x3E
        DICTUADDGETB                         => 0xF4, 0x57
        DICTUADDGETREF                       => 0xF4, 0x3F
        DICTUADDREF                          => 0xF4, 0x37
        DICTUDEL                             => 0xF4, 0x5B
        DICTUDELGET                          => 0xF4, 0x66
        DICTUDELGETREF                       => 0xF4, 0x67
        DICTUGET                             => 0xF4, 0x0E
        DICTUGETEXEC                         => 0xF4, 0xA3
        DICTUGETEXECZ                        => 0xF4, 0xBF
        DICTUGETJMP                          => 0xF4, 0xA1
        DICTUGETJMPZ                         => 0xF4, 0xBD
        DICTUGETNEXT                         => 0xF4, 0x7C
        DICTUGETNEXTEQ                       => 0xF4, 0x7D
        DICTUGETPREV                         => 0xF4, 0x7E
        DICTUGETPREVEQ                       => 0xF4, 0x7F
        DICTUGETREF                          => 0xF4, 0x0F
        DICTUMAX                             => 0xF4, 0x8E
        DICTUMAXREF                          => 0xF4, 0x8F
        DICTUMIN                             => 0xF4, 0x86
        DICTUMINREF                          => 0xF4, 0x87
        DICTUREMMAX                          => 0xF4, 0x9E
        DICTUREMMAXREF                       => 0xF4, 0x9F
        DICTUREMMIN                          => 0xF4, 0x96
        DICTUREMMINREF                       => 0xF4, 0x97
        DICTUREPLACE                         => 0xF4, 0x26
        DICTUREPLACEB                        => 0xF4, 0x4B
        DICTUREPLACEGET                      => 0xF4, 0x2E
        DICTUREPLACEGETB                     => 0xF4, 0x4F
        DICTUREPLACEGETREF                   => 0xF4, 0x2F
        DICTUREPLACEREF                      => 0xF4, 0x27
        DICTUSET                             => 0xF4, 0x16
        DICTUSETB                            => 0xF4, 0x43
        DICTUSETGET                          => 0xF4, 0x1E
        DICTUSETGETB                         => 0xF4, 0x47
        DICTUSETGETREF                       => 0xF4, 0x1F
        DICTUSETREF                          => 0xF4, 0x17
        DIV                                  => 0xA9, 0x04
        DIVC                                 => 0xA9, 0x06
        DIVR                                 => 0xA9, 0x05
        DIVMOD                               => 0xA9, 0x0C
        DIVMODC                              => 0xA9, 0x0E
        DIVMODR                              => 0xA9, 0x0D
        DROP                                 => 0x30
        DROPX                                => 0x65
        DROP2                                => 0x5B
        DUEPAYMENT                           => 0xF8, 0x2F
        DUMP z = parse_const_u4_14           => 0xFE, 0x20 | z
        DUMPSTK                              => 0xFE, 0x00
        DUMPSTKTOP z = parse_const_u4_1_14   => 0xFE, z
        DUP                                  => 0x20
        DUP2                                 => 0x5C
        ECRECOVER                            => 0xF9, 0x12
        ENDC                                 => 0xC9
        ENDCST                               => 0xCD
        ENDXC                                => 0xCF, 0x23
        ENDS                                 => 0xD1
        EQUAL                                => 0xBA
        EQINT z = parse_const_i8             => 0xC0, z
        EXECUTE                              => 0xD8
        EXPLODE c = parse_const_u4           => 0x6F, 0x40 | c
        EXPLODEVAR                           => 0x6F, 0x84
        FALSE                                => 0x70
        FIRST                                => 0x6F, 0x10
        FITS z = parse_const_u8_plus_one     => 0xB4, z
        FITSX                                => 0xB6, 0x00
        GASCONSUMED                          => 0xF8, 0x07
        GASTOGRAM                            => 0xF8, 0x05
        GEQ                                  => 0xBE
        GETEXTRABALANCE                      => 0xf8, 0x80
        GETFORWARDFEE                        => 0xF8, 0x38
        GETFORWARDFEESIMPLE                  => 0xF8, 0x3C
        GETGASFEE                            => 0xF8, 0x36
        GETGASFEESIMPLE                      => 0xF8, 0x3B
        GETGLOBVAR                           => 0xF8, 0x40
        GETGLOB k = parse_const_u5           => 0xF8, 0x40 | k
        GETORIGINALFWDFEE                    => 0xF8, 0x3A
        GETPARAM c = parse_const_u4          => 0xF8, 0x20 | c
        GETPARAMLONG c = parse_const_u8 => 0xF8, 0x81, c
        GETPRECOMPILEDGAS                    => 0xF8, 0x39
        GETSTORAGEFEE                        => 0xF8, 0x37
        GLOBALID                             => 0xF8, 0x35
        GRAMTOGAS                            => 0xF8, 0x04
        GREATER                              => 0xBC
        GTINT z = parse_const_i8             => 0xC2, z
        HASHBU                               => 0xF9, 0x16
        HASHCU                               => 0xF9, 0x00
        HASHSU                               => 0xF9, 0x01
        HASHEXT z = parse_const_u8_0_4       => 0xF9, 0x04, z
        HASHEXTR z = parse_const_u8_0_4      => 0xF9, 0x05, z
        HASHEXTA z = parse_const_u8_0_4      => 0xF9, 0x06, z
        HASHEXTAR z = parse_const_u8_0_4     => 0xF9, 0x07, z
        HASHEXT_SHA256                       => 0xF9, 0x04, 0x00
        HASHEXT_SHA512                       => 0xF9, 0x04, 0x01
        HASHEXT_BLAKE2B                      => 0xF9, 0x04, 0x02
        HASHEXT_KECCAK256                    => 0xF9, 0x04, 0x03
        HASHEXT_KECCAK512                    => 0xF9, 0x04, 0x04
        HASHEXTR_SHA256                      => 0xF9, 0x05, 0x00
        HASHEXTR_SHA512                      => 0xF9, 0x05, 0x01
        HASHEXTR_BLAKE2B                     => 0xF9, 0x05, 0x02
        HASHEXTR_KECCAK256                   => 0xF9, 0x05, 0x03
        HASHEXTR_KECCAK512                   => 0xF9, 0x05, 0x04
        HASHEXTA_SHA256                      => 0xF9, 0x06, 0x00
        HASHEXTA_SHA512                      => 0xF9, 0x06, 0x01
        HASHEXTA_BLAKE2B                     => 0xF9, 0x06, 0x02
        HASHEXTA_KECCAK256                   => 0xF9, 0x06, 0x03
        HASHEXTA_KECCAK512                   => 0xF9, 0x06, 0x04
        HASHEXTAR_SHA256                     => 0xF9, 0x07, 0x00
        HASHEXTAR_SHA512                     => 0xF9, 0x07, 0x01
        HASHEXTAR_BLAKE2B                    => 0xF9, 0x07, 0x02
        HASHEXTAR_KECCAK256                  => 0xF9, 0x07, 0x03
        HASHEXTAR_KECCAK512                  => 0xF9, 0x07, 0x04
        IF                                   => 0xDE
        IFBITJMP n = parse_const_u5          => 0xE3, 0x80 | n
        IFBITJMPREF n = parse_const_u5       => 0xE3, 0xC0 | n
        IFELSE                               => 0xE2
        IFJMP                                => 0xE0
        IFNBITJMP n = parse_const_u5         => 0xE3, 0xA0 | n
        IFNBITJMPREF n = parse_const_u5      => 0xE3, 0xE0 | n
        IFNOT                                => 0xDF
        IFNOTJMP                             => 0xE1
        IFNOTRET                             => 0xDD
        IFNOTRETALT                          => 0xE3, 0x09
        IFRET                                => 0xDC
        IFRETALT                             => 0xE3, 0x08
        INC                                  => 0xA4
        INCOMINGVALUE                        => 0xF8, 0x2B
        INMSGPARAMS                          => 0xf8, 0x81, 0x11
        INMSG_BOUNCE                         => 0xf8, 0x90
        INMSG_BOUNCED                        => 0xf8, 0x91
        INMSG_SRC                            => 0xf8, 0x92
        INMSG_FWDFEE                         => 0xf8, 0x93
        INMSG_LT                             => 0xf8, 0x94
        INMSG_UTIME                          => 0xf8, 0x95
        INMSG_ORIGVALUE                      => 0xf8, 0x96
        INMSG_VALUE                          => 0xf8, 0x97
        INMSG_VALUEEXTRA                     => 0xf8, 0x98
        INMSG_STATEINIT                      => 0xf8, 0x99
        INMSGPARAM c = parse_const_u4         => 0xf8, 0x90 | c
        INTSORT2                             => 0xB6, 0x0A
        INVERT                               => 0xED, 0xF8
        INDEX c = parse_const_u4             => 0x6F, 0x10 | c
        INDEXQ c = parse_const_u4            => 0x6F, 0x60 | c
        INDEXVAR                             => 0x6F, 0x81
        INDEXVARQ                            => 0x6F, 0x86
        INDEX2 i = parse_const_u2;
               j = parse_const_u2            => 0x6F, 0xB0 | (i << 2) | j
        INDEX3 i = parse_const_u2;
               j = parse_const_u2;
               k = parse_const_u2            => 0x6F, 0xC0 | (i << 4) | (j << 2) | k
        ISNAN                                => 0xC4
        ISNEG                                => 0xC1, 0x00
        ISNPOS                               => 0xC1, 0x01
        ISNNEG                               => 0xC2, 0xFF
        ISNULL                               => 0x6E
        ISPOS                                => 0xC2, 0x00
        ISTUPLE                              => 0x6F, 0x8A
        ISZERO                               => 0xC0, 0x00
        JMP n = parse_const_u14              => 0xF1, 0x40 | ((n / 256) as u8), ((n % 256) as u8)
        JMPX                                 => 0xD9
        JMPXARGS p = parse_const_u4          => 0xDB, 0x10 | p
        JMPXDATA                             => 0xDB, 0x35
        JMPXVARARGS                          => 0xDB, 0x3A
        JMPREFDATA                           => 0xDB, 0x3E
        HEXDUMP                              => 0xFE, 0x10
        HEXPRINT                             => 0xFE, 0x11
        LAST                                 => 0x6F, 0x8B
        LDI cc = parse_const_u8_plus_one     => 0xD2, cc
        LDDICT                               => 0xF4, 0x04
        LDDICTS                              => 0xF4, 0x02
        LDDICTQ                              => 0xF4, 0x06
        LDGRAMS                              => 0xFA, 0x00
        LDILE4                               => 0xD7, 0x50
        LDILE4Q                              => 0xD7, 0x58
        LDILE8                               => 0xD7, 0x52
        LDILE8Q                              => 0xD7, 0x5A
        LDIQ cc = parse_const_u8_plus_one    => 0xD7, 0x0C, cc
        LDIX                                 => 0xD7, 0x00
        LDIXQ                                => 0xD7, 0x04
        LDMSGADDR                            => 0xFA, 0x40
        LDMSGADDRQ                           => 0xFA, 0x41
        LDONES                               => 0xD7, 0x61
        LDOPTREF                             => 0xF4, 0x04
        LDOPTSTDADDR                         => 0xFA, 0x50
        LDOPTSTDADDRQ                        => 0xFA, 0x51
        LDREF                                => 0xD4
        LDREFRTOS                            => 0xD5
        LDSAME                               => 0xD7, 0x62
        LDSLICE cc = parse_const_u8_plus_one => 0xD6, cc
        LDSLICEQ
            cc = parse_const_u8_plus_one     => 0xD7, 0x1E, cc
        LDSLICEX                             => 0xD7, 0x18
        LDSLICEXQ                            => 0xD7, 0x1A
        LDSTDADDR                            => 0xFA, 0x48
        LDSTDADDRQ                           => 0xFA, 0x49
        LDU z = parse_const_u8_plus_one      => 0xD3, z
        LDULE4                               => 0xD7, 0x51
        LDULE4Q                              => 0xD7, 0x59
        LDULE8                               => 0xD7, 0x53
        LDULE8Q                              => 0xD7, 0x5B
        LDUQ cc = parse_const_u8_plus_one    => 0xD7, 0x0D, cc
        LDUX                                 => 0xD7, 0x01
        LDUXQ                                => 0xD7, 0x05
        LDVARINT16                           => 0xFA, 0x01
        LDVARINT32                           => 0xFA, 0x05
        LDVARUINT16                          => 0xFA, 0x00
        LDVARUINT32                          => 0xFA, 0x04
        LDZEROES                             => 0xD7, 0x60
        LEQ                                  => 0xBB
        LESS                                 => 0xB9
        LESSINT z = parse_const_i8           => 0xC1, z
        LOGFLUSH                             => 0xFE, 0xF0, 0x00
        LTIME                                => 0xF8, 0x25
        MAX                                  => 0xB6, 0x09
        MIN                                  => 0xB6, 0x08
        MINMAX                               => 0xB6, 0x0A
        MOD                                  => 0xA9, 0x08
        MODC                                 => 0xA9, 0x0A
        MODR                                 => 0xA9, 0x09
        MUL                                  => 0xA8
        MULCONST z = parse_const_i8          => 0xA7, z
        MULDIV                               => 0xA9, 0x84
        MULDIVC                              => 0xA9, 0x86
        MULDIVR                              => 0xA9, 0x85
        MULDIVMOD                            => 0xA9, 0x8C
        MULDIVMODC                           => 0xA9, 0x8E
        MULDIVMODR                           => 0xA9, 0x8D
        MULMOD                               => 0xA9, 0x88
        MULMODC                              => 0xA9, 0x8A
        MULMODR                              => 0xA9, 0x89
        MYADDR                               => 0xF8, 0x28
        MYCODE                               => 0xF8, 0x2A
        NEGATE                               => 0xA3
        NEQ                                  => 0xBD
        NEQINT z = parse_const_i8            => 0xC3, z
        NEWC                                 => 0xC8
        NEWDICT                              => 0x6D
        NIL                                  => 0x6F, 0x00
        NIP                                  => 0x31
        NOP                                  => 0x00
        NOT                                  => 0xB3
        NOW                                  => 0xF8, 0x23
        NULL                                 => 0x6D
        NULLROTRIF                           => 0x6F, 0xA2
        NULLROTRIF2                          => 0x6F, 0xA6
        NULLROTRIFNOT                        => 0x6F, 0xA3
        NULLROTRIFNOT2                       => 0x6F, 0xA7
        NULLSWAPIF                           => 0x6F, 0xA0
        NULLSWAPIF2                          => 0x6F, 0xA4
        NULLSWAPIFNOT                        => 0x6F, 0xA1
        NULLSWAPIFNOT2                       => 0x6F, 0xA5
        ONE                                  => 0x71
        OR                                   => 0xB1
        OVER                                 => 0x21
        OVER2                                => 0x5D
        ONLYTOPX                             => 0x6A
        ONLYX                                => 0x6B
        P256_CHKSIGNU                        => 0xF9, 0x14
        P256_CHKSIGNS                        => 0xF9, 0x15
        PAIR                                 => 0x6F, 0x02
        PARSEMSGADDR                         => 0xFA, 0x42
        PARSEMSGADDRQ                        => 0xFA, 0x43
        PFXDICTADD                           => 0xF4, 0x72
        PFXDICTCONSTGETJMP n = parse_const_u10 => 0xF4, 0xAC | (n >> 8) as u8, n as u8
        PFXDICTDEL                           => 0xF4, 0x73
        PFXDICTGET                           => 0xF4, 0xA9
        PFXDICTGETEXEC                       => 0xF4, 0xAB
        PFXDICTGETJMP                        => 0xF4, 0xAA
        PFXDICTGETQ                          => 0xF4, 0xA8
        PFXDICTREPLACE                       => 0xF4, 0x71
        PFXDICTSET                           => 0xF4, 0x70
        PFXDICTSWITCH n = parse_const_u10 => 0xF4, 0xAC | (n >> 8) as u8, n as u8
        PLDDICT                              => 0xF4, 0x05
        PLDDICTS                             => 0xF4, 0x03
        PLDDICTQ                             => 0xF4, 0x07
        PLDI cc = parse_const_u8_plus_one    => 0xD7, 0x0A, cc
        PLDILE4                              => 0xD7, 0x54
        PLDILE4Q                             => 0xD7, 0x5C
        PLDILE8                              => 0xD7, 0x56
        PLDILE8Q                             => 0xD7, 0x5E
        PLDIQ cc = parse_const_u8_plus_one   => 0xD7, 0x0E, cc
        PLDIX                                => 0xD7, 0x02
        PLDIXQ                               => 0xD7, 0x06
        PLDSLICE
            cc = parse_const_u8_plus_one     => 0xD7, 0x1D, cc
        PLDSLICEQ
            cc = parse_const_u8_plus_one     => 0xD7, 0x1F, cc
        PLDOPTREF                            => 0xF4, 0x05
        PLDREF                               => 0xD7, 0x4C
        PLDREFVAR                            => 0xD7, 0x48
        PLDREFIDX n = parse_const_u2         => 0xD7, 0x4C | n
        PLDSLICEX                            => 0xD7, 0x19
        PLDSLICEXQ                           => 0xD7, 0x1B
        PLDU cc = parse_const_u8_plus_one    => 0xD7, 0x0B, cc
        PLDULE4                              => 0xD7, 0x55
        PLDULE4Q                             => 0xD7, 0x5D
        PLDULE8                              => 0xD7, 0x57
        PLDULE8Q                             => 0xD7, 0x5F
        PLDUQ cc = parse_const_u8_plus_one   => 0xD7, 0x0F, cc
        PLDUX                                => 0xD7, 0x03
        PLDUXQ                               => 0xD7, 0x07
        PLDUZ c = parse_plduz_parameter      => 0xD7, 0x10 | c
        PICK                                 => 0x60
        PUSHX                                => 0x60
        POPCTR z = parse_control_register    => 0xED, 0x50 | z
        POPCTRSAVE z=parse_control_register  => 0xED, 0x90 | z
        POPCTRX                              => 0xED, 0xE1
        POPROOT                              => 0xED, 0x54
        POPSAVE z = parse_control_register   => 0xED, 0x90 | z
        POW2                                 => 0xAE
        PREPARE n = parse_const_u14          => 0xF1, 0x80 | ((n / 256) as u8), ((n % 256) as u8)
        PREPAREDICT n = parse_const_u14      => 0xF1, 0x80 | ((n / 256) as u8), ((n % 256) as u8)
        PREVBLOCKSINFOTUPLE                  => 0xF8, 0x2D
        PREVMCBLOCKS                         => 0xF8, 0x34, 0x00
        PREVKEYBLOCK                         => 0xF8, 0x34, 0x01
        PREVMCBLOCKS_100                     => 0xF8, 0x34, 0x02
        PRINT z = parse_const_u4_14          => 0xFE, 0x30 | z
        PU2XC
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4_minus_one;
            s3 = parse_stack_register_u4_minus_two
                                             => 0x54, 0x60 | s1, (s2 << 4) | s3
        PUSH2
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4     => 0x53, (s1 << 4) | s2
        PUSH3
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4;
            s3 = parse_stack_register_u4     => 0x54, 0x70 | s1, (s2 << 4) | s3
        PUSHCTR z = parse_control_register   => 0xED, 0x40 | z
        PUSHCTRX                             => 0xED, 0xE0
        PUSHNAN                              => 0x83, 0xFF
        PUSHNEGPOW2
            s1 = parse_const_u8_plus_one     => 0x85, s1
        PUSHNULL                             => 0x6D
        PUSHPOW2
            s1 = parse_const_u8_plus_one     => 0x83, s1
        PUSHPOW2DEC
            s1 = parse_const_u8_plus_one     => 0x84, s1
        PUSHROOT                             => 0xED, 0x44
        PUXC
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4_minus_one
                                             => 0x52, (s1 << 4) | s2
        PUXC2
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4_minus_one;
            s3 = parse_stack_register_u4_minus_one
                                             => 0x54, 0x40 | s1, (s2 << 4) | s3
        PUXCPU
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4_minus_one;
            s3 = parse_stack_register_u4_minus_one
                                             => 0x54, 0x50 | s1, (s2 << 4) | s3
        QABS                                 => 0xB7, 0xB6, 0x0B
        QADD                                 => 0xB7, 0xA0
        QADDCONST z = parse_const_i8         => 0xB7, 0xA6, z
        QAND                                 => 0xB7, 0xB0
        QBITSIZE                             => 0xB7, 0xB6, 0x02
        QCMP                                 => 0xB7, 0xBF
        QDEC                                 => 0xB7, 0xA5
        QDIV                                 => 0xB7, 0xA9, 0x04
        QDIVC                                => 0xB7, 0xA9, 0x06
        QDIVR                                => 0xB7, 0xA9, 0x05
        QDIVMOD                              => 0xB7, 0xA9, 0x0C
        QDIVMODC                             => 0xB7, 0xA9, 0x0E
        QDIVMODR                             => 0xB7, 0xA9, 0x0D
        QEQINT z = parse_const_i8            => 0xB7, 0xC0, z
        QEQUAL                               => 0xB7, 0xBA
        QFITS z = parse_const_u8_plus_one    => 0xB7, 0xB4, z
        QFITSX                               => 0xB7, 0xB6, 0x00
        QGEQ                                 => 0xB7, 0xBE
        QGREATER                             => 0xB7, 0xBC
        QGTINT z = parse_const_i8            => 0xB7, 0xC2, z
        QINC                                 => 0xB7, 0xA4
        QINTSORT2                            => 0xB7, 0xB6, 0x0A
        QMAX                                 => 0xB7, 0xB6, 0x09
        QMIN                                 => 0xB7, 0xB6, 0x08
        QMINMAX                              => 0xB7, 0xB6, 0x0A
        QMOD                                 => 0xB7, 0xA9, 0x08
        QMODC                                => 0xB7, 0xA9, 0x0A
        QMODR                                => 0xB7, 0xA9, 0x09
        QMUL                                 => 0xB7, 0xA8
        QMULCONST z = parse_const_i8         => 0xB7, 0xA7, z
        QMULDIV                              => 0xB7, 0xA9, 0x84
        QMULDIVC                             => 0xB7, 0xA9, 0x86
        QMULDIVR                             => 0xB7, 0xA9, 0x85
        QMULDIVMOD                           => 0xB7, 0xA9, 0x8C
        QMULDIVMODC                          => 0xB7, 0xA9, 0x8E
        QMULDIVMODR                          => 0xB7, 0xA9, 0x8D
        QMULMOD                              => 0xB7, 0xA9, 0x88
        QMULMODC                             => 0xB7, 0xA9, 0x8A
        QMULMODR                             => 0xB7, 0xA9, 0x89
        QLESS                                => 0xB7, 0xB9
        QLESSINT z = parse_const_i8          => 0xB7, 0xC1, z
        QLEQ                                 => 0xB7, 0xBB
        QNEGATE                              => 0xB7, 0xA3
        QNEQ                                 => 0xB7, 0xBD
        QNEQINT z = parse_const_i8           => 0xB7, 0xC3, z
        QNOT                                 => 0xB7, 0xB3
        QOR                                  => 0xB7, 0xB1
        QPOW2                                => 0xB7, 0xAE
        QSGN                                 => 0xB7, 0xB8
        QSUB                                 => 0xB7, 0xA1
        QSUBR                                => 0xB7, 0xA2
        QTLEN                                => 0x6F, 0x89
        QUBITSIZE                            => 0xB7, 0xB6, 0x03
        QUFITS z = parse_const_u8_plus_one   => 0xB7, 0xB5, z
        QUFITSX                              => 0xB7, 0xB6, 0x01
        QXOR                                 => 0xB7, 0xB2
        RAND                                 => 0xF8, 0x11
        RANDSEED                             => 0xF8, 0x26
        RANDU256                             => 0xF8, 0x10
        RAWRESERVE                           => 0xFB, 0x02
        RAWRESERVEX                          => 0xFB, 0x03
        REPEAT                               => 0xE4
        REPEATBRK                            => 0xE3, 0x14
        REPEATEND                            => 0xE5
        REPEATENDBRK                         => 0xE3, 0x15
        RET                                  => 0xDB, 0x30
        RETALT                               => 0xDB, 0x31
        RETARGS r = parse_const_u4           => 0xDB, 0x20 | r
        RETBOOL                              => 0xDB, 0x32
        RETDATA                              => 0xDB, 0x3F
        RETFALSE                             => 0xDB, 0x31
        RETTRUE                              => 0xDB, 0x30
        RETURNARGS c = parse_const_u4        => 0xED, c
        RETURNVARARGS                        => 0xED, 0x10
        RETVARARGS                           => 0xDB, 0x39
        REVERSE c1 = parse_const_u4_plus_two;
                c2 = parse_const_u4          => 0x5E, (c1 << 4) | c2
        REVX                                 => 0x64
        REWRITESTDADDR                       => 0xFA, 0x44
        REWRITESTDADDRQ                      => 0xFA, 0x45
        REWRITEVARADDR                       => 0xFA, 0x46
        REWRITEVARADDRQ                      => 0xFA, 0x47
        RIST255_FROMHASH                     => 0xF9, 0x20
        RIST255_VALIDATE                     => 0xF9, 0x21
        RIST255_ADD                          => 0xF9, 0x22
        RIST255_SUB                          => 0xF9, 0x23
        RIST255_MUL                          => 0xF9, 0x24
        RIST255_MULBASE                      => 0xF9, 0x25
        RIST255_PUSHL                        => 0xF9, 0x26
        RIST255_QVALIDATE                    => 0xB7, 0xF9, 0x21
        RIST255_QADD                         => 0xB7, 0xF9, 0x22
        RIST255_QSUB                         => 0xB7, 0xF9, 0x23
        RIST255_QMUL                         => 0xB7, 0xF9, 0x24
        RIST255_QMULBASE                     => 0xB7, 0xF9, 0x25
        ROT                                  => 0x58
        ROT2                                 => 0x55, 0x13
        ROTREV                               => 0x59
        ROLL c = parse_const_u4_plus_one     => 0x55, c
        ROLLREV c = parse_const_u4_plus_one  => 0x55, c << 4
        ROLLX                                => 0x61
        ROLLREVX                             => 0x62
        RUNVM n = parse_const_u12            => 0xDB, 0x40 | ((n / 256) as u8), ((n % 256) as u8)
        RUNVMX                               => 0xDB, 0x50
        SAMEALT                              => 0xED, 0xFA
        SAMEALTSAVE                          => 0xED, 0xFB
        SAVE z = parse_control_register      => 0xED, 0xA0 | z
        SAVEALT z = parse_control_register   => 0xED, 0xB0 | z
        SAVEALTCTR z=parse_control_register  => 0xED, 0xB0 | z
        SAVEBOTH z = parse_control_register  => 0xED, 0xC0 | z
        SAVEBOTHCTR z=parse_control_register => 0xED, 0xC0 | z
        SAVECTR z = parse_control_register   => 0xED, 0xA0 | z
        SBITS                                => 0xD7, 0x49
        SBITREFS                             => 0xD7, 0x4B
        SCHKBITS                             => 0xD7, 0x41
        SCHKBITREFS                          => 0xD7, 0x43
        SCHKBITREFSQ                         => 0xD7, 0x47
        SCHKBITSQ                            => 0xD7, 0x45
        SCHKREFS                             => 0xD7, 0x42
        SCHKREFSQ                            => 0xD7, 0x46
        SCUTFIRST                            => 0xD7, 0x30
        SCUTLAST                             => 0xD7, 0x32
        SDATASIZE                            => 0xF9, 0x43
        SDATASIZEQ                           => 0xF9, 0x42
        SDBEGINSX                            => 0xD7, 0x26
        SDBEGINSXQ                           => 0xD7, 0x27
        SDCNTLEAD0                           => 0xC7, 0x10
        SDCNTLEAD1                           => 0xC7, 0x11
        SDCNTTRAIL0                          => 0xC7, 0x12
        SDCNTTRAIL1                          => 0xC7, 0x13
        SDCUTFIRST                           => 0xD7, 0x20
        SDCUTLAST                            => 0xD7, 0x22
        SDEMPTY                              => 0xC7, 0x01
        SDEQ                                 => 0xC7, 0x05
        SDFIRST                              => 0xC7, 0x03
        SDEPTH                               => 0xD7, 0x64
        SDPFX                                => 0xC7, 0x08
        SDPFXREV                             => 0xC7, 0x09
        SDPPFX                               => 0xC7, 0x0A
        SDPPFXREV                            => 0xC7, 0x0B
        SDPSFX                               => 0xC7, 0x0E
        SDPSFXREV                            => 0xC7, 0x0F
        SDSFX                                => 0xC7, 0x0C
        SDSFXREV                             => 0xC7, 0x0D
        SDLEXCMP                             => 0xC7, 0x04
        SDSKIPFIRST                          => 0xD7, 0x21
        SDSKIPLAST                           => 0xD7, 0x23
        SDSUBSTR                             => 0xD7, 0x24
        SECOND                               => 0x6F, 0x11
        SECP256K1_XONLY_PUBKEY_TWEAK_ADD      => 0xF9, 0x13
        SEMPTY                               => 0xC7, 0x00
        SENDMSG                              => 0xFB, 0x08
        SENDRAWMSG                           => 0xFB, 0x00
        SETALTCTR z = parse_control_register => 0xED, 0x80 | z
        SETCODE                              => 0xFB, 0x04
        SETCONT z = parse_control_register   => 0xED, 0x60 | z
        SETCONTCTR z=parse_control_register  => 0xED, 0x60 | z
        SETCONTCTRX                          => 0xED, 0xE2
        SETCONTVARARGS                       => 0xED, 0x11
        SETCP z = parse_const_u8_setcp       => 0xFF, z
        SETCP0                               => 0xFF, 0x00
        SETCPX                               => 0xFF, 0xF0
        SETEXITALT                           => 0xED, 0xF5
        SETCONTCTRMANY c = parse_const_u8    => 0xED, 0xE3, c
        SETCONTCTRMANYX                      => 0xED, 0xE4
        SETGASLIMIT                          => 0xF8, 0x01
        SETGLOBVAR                           => 0xF8, 0x60
        SETGLOB k = parse_const_u5           => 0xF8, 0x60 | k
        SETFIRST                             => 0x6F, 0x50
        SETINDEX c = parse_const_u4          => 0x6F, 0x50 | c
        SETINDEXQ c = parse_const_u4         => 0x6F, 0x70 | c
        SETINDEXVAR                          => 0x6F, 0x85
        SETINDEXVARQ                         => 0x6F, 0x87
        SETLIBCODE                           => 0xFB, 0x06
        SETNUMARGS c = parse_const_u4_14     => 0xEC, c
        SETNUMVARARGS                        => 0xED, 0x12
        SETRAND                              => 0xF8, 0x14
        SETRETCTR z = parse_control_register => 0xED, 0x70 | z
        SETSECOND                            => 0x6F, 0x51
        SETTHIRD                             => 0x6F, 0x52
        SGN                                  => 0xB8
        SHA256U                              => 0xF9, 0x02
        SINGLE                               => 0x6F, 0x01
        SKIPDICT                             => 0xF4, 0x01
        SKIPOPTREF                           => 0xF4, 0x01
        SPLIT                                => 0xD7, 0x36
        SPLITQ                               => 0xD7, 0x37
        SREFS                                => 0xD7, 0x4A
        SREMPTY                              => 0xC7, 0x02
        SSKIPFIRST                           => 0xD7, 0x31
        SSKIPLAST                            => 0xD7, 0x33
        STB                                  => 0xCF, 0x13
        STBQ                                 => 0xCF, 0x1B
        STBR                                 => 0xCF, 0x17
        STBREF                               => 0xCF, 0x11
        STBREFQ                              => 0xCF, 0x19
        STBREFR                              => 0xCD
        STBREFRQ                             => 0xCF, 0x1D
        STBRQ                                => 0xCF, 0x1F
        STGRAMS                              => 0xFA, 0x02
        STDICT                               => 0xF4, 0x00
        STDICTS                              => 0xCE
        STI z = parse_const_u8_plus_one      => 0xCA, z
        STILE4                               => 0xCF, 0x28
        STILE8                               => 0xCF, 0x2A
        STIQ z = parse_const_u8_plus_one     => 0xCF, 0x0C, z
        STIR z = parse_const_u8_plus_one     => 0xCF, 0x0A, z
        STIRQ z = parse_const_u8_plus_one    => 0xCF, 0x0E, z
        STIX                                 => 0xCF, 0x00
        STIXQ                                => 0xCF, 0x04
        STIXR                                => 0xCF, 0x02
        STIXRQ                               => 0xCF, 0x06
        STONE                                => 0xCF, 0x83
        STONES                               => 0xCF, 0x41
        STOPTREF                             => 0xF4, 0x00
        STOPTSTDADDR                         => 0xFA, 0x54
        STOPTSTDADDRQ                        => 0xFA, 0x55
        STORAGEFEES                          => 0xF8, 0x2C
        STRDUMP                              => 0xFE, 0x14
        STRPRINT                             => 0xFE, 0x15
        STREF                                => 0xCC
        STREF2CONST                          => 0xCF, 0x21
        STREF3CONST                          => 0xCF, 0xE2
        STREFCONST                           => 0xCF, 0x20
        STREFQ                               => 0xCF, 0x18
        STREFR                               => 0xCF, 0x14
        STREFRQ                              => 0xCF, 0x1C
        STSAME                               => 0xCF, 0x42
        STSLICE                              => 0xCE
        STSLICEQ                             => 0xCF, 0x1A
        STSLICER                             => 0xCF, 0x16
        STSLICERQ                            => 0xCF, 0x1E
        STSTDADDR                            => 0xFA, 0x52
        STSTDADDRQ                           => 0xFA, 0x53
        STU z = parse_const_u8_plus_one      => 0xCB, z
        STULE4                               => 0xCF, 0x29
        STULE8                               => 0xCF, 0x2B
        STUQ z = parse_const_u8_plus_one     => 0xCF, 0x0D, z
        STUR z = parse_const_u8_plus_one     => 0xCF, 0x0B, z
        STURQ z = parse_const_u8_plus_one    => 0xCF, 0x0F, z
        STUX                                 => 0xCF, 0x01
        STUXQ                                => 0xCF, 0x05
        STUXR                                => 0xCF, 0x03
        STUXRQ                               => 0xCF, 0x07
        STVARINT16                           => 0xFA, 0x03
        STVARINT32                           => 0xFA, 0x07
        STVARUINT16                          => 0xFA, 0x02
        STVARUINT32                          => 0xFA, 0x06
        STZERO                               => 0xCF, 0x81
        STZEROES                             => 0xCF, 0x40
        SUB                                  => 0xA1
        SUBDICTGET                           => 0xF4, 0xB1
        SUBDICTIGET                          => 0xF4, 0xB2
        SUBDICTUGET                          => 0xF4, 0xB3
        SUBDICTIRPGET                        => 0xF4, 0xB6
        SUBDICTRPGET                         => 0xF4, 0xB5
        SUBDICTURPGET                        => 0xF4, 0xB7
        SUBR                                 => 0xA2
        SUBSLICE                             => 0xD7, 0x34
        SWAP2                                => 0x5A
        TEN                                  => 0x7A
        THENRET                              => 0xED, 0xF6
        THENRETALT                           => 0xED, 0xF7
        THIRD                                => 0x6F, 0x12
        THROWANY                             => 0xF2, 0xF0
        THROWANYIF                           => 0xF2, 0xF2
        THROWANYIFNOT                        => 0xF2, 0xF4
        THROWARG n = parse_const_u11         => 0xF2, 0xC8 | ((n / 256) as u8), ((n % 256) as u8)
        THROWARGANY                          => 0xF2, 0xF1
        THROWARGANYIF                        => 0xF2, 0xF3
        THROWARGANYIFNOT                     => 0xF2, 0xF5
        THROWARGIF n = parse_const_u11       => 0xF2, 0xD8 | ((n / 256) as u8), ((n % 256) as u8)
        THROWARGIFNOT n = parse_const_u11    => 0xF2, 0xE8 | ((n / 256) as u8), ((n % 256) as u8)
        TLEN                                 => 0x6F, 0x88
        TPOP                                 => 0x6F, 0x8D
        TPUSH                                => 0x6F, 0x8C
        TRIPLE                               => 0x6F, 0x03
        TRUE                                 => 0x7F
        TRY                                  => 0xF2, 0xFF
        TRYARGS s1 = parse_const_u4;
                s2 = parse_const_u4          => 0xF3, ((s1 << 4) | s2)
        TUCK                                 => 0x66
        TUPLE s = parse_const_u4             => 0x6F, s
        TUPLEVAR                             => 0x6F, 0x80
        TWO                                  => 0x72
        UBITSIZE                             => 0xB6, 0x03
        UFITS z = parse_const_u8_plus_one    => 0xB5, z
        UFITSX                               => 0xB6, 0x01
        UNCONS                               => 0x6F, 0x22
        UNPACKEDCONFIGTUPLE                  => 0xF8, 0x2E
        UNPACKFIRST c = parse_const_u4       => 0x6F, 0x30 | c
        UNPACKFIRSTVAR                       => 0x6F, 0x83
        UNPAIR                               => 0x6F, 0x22
        UNSINGLE                             => 0x6F, 0x21
        UNTIL                                => 0xE6
        UNTILBRK                             => 0xE3, 0x16
        UNTILEND                             => 0xE7
        UNTILENDBRK                          => 0xE3, 0x17
        UNTRIPLE                             => 0x6F, 0x23
        UNTUPLE c = parse_const_u4           => 0x6F, 0x20 | c
        UNTUPLEVAR                           => 0x6F, 0x82
        WHILE                                => 0xE8
        WHILEBRK                             => 0xE3, 0x18
        WHILEEND                             => 0xE9
        WHILEENDBRK                          => 0xE3, 0x19
        XC2PU
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4;
            s3 = parse_stack_register_u4     => 0x54, 0x10 | s1, (s2 << 4) | s3
        XCHG2
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4     => 0x50, (s1 << 4) | s2
        XCHG3
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4;
            s3 = parse_stack_register_u4     => 0x40 | s1, (s2 << 4) | s3
        XCHGX                                => 0x67
        XCPU
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4     => 0x51, (s1 << 4) | s2
        XCPU2
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4;
            s3 = parse_stack_register_u4     => 0x54, 0x30 | s1, (s2 << 4) | s3
        XCPUXC
            s1 = parse_stack_register_u4;
            s2 = parse_stack_register_u4;
            s3 = parse_stack_register_u4_minus_one
                                             => 0x54, 0x20 | s1, (s2 << 4) | s3
        XCTOS                                => 0xD7, 0x39
        XLOAD                                => 0xD7, 0x3A
        XLOADQ                               => 0xD7, 0x3B
        XOR                                  => 0xB2
        ZERO                                 => 0x70
    }

    simple_commands! {
        enumerate_ton_commands

        MULADDDIVMOD                         => 0xA9, 0x80
        MULADDDIVMODR                        => 0xA9, 0x81
        MULADDDIVMODC                        => 0xA9, 0x82
        QMULADDDIVMOD                        => 0xB7, 0xA9, 0x80
        QMULADDDIVMODR                       => 0xB7, 0xA9, 0x81
        QMULADDDIVMODC                       => 0xB7, 0xA9, 0x82

        ADDDIVMOD                             => 0xA9, 0x00
        ADDDIVMODR                            => 0xA9, 0x01
        ADDDIVMODC                            => 0xA9, 0x02
        QADDDIVMOD                            => 0xB7, 0xA9, 0x00
        QADDDIVMODR                           => 0xB7, 0xA9, 0x01
        QADDDIVMODC                           => 0xB7, 0xA9, 0x02

        ADDRSHIFTMOD                          => 0xA9, 0x20
        ADDRSHIFTMODR                         => 0xA9, 0x21
        ADDRSHIFTMODC                         => 0xA9, 0x22
        QADDRSHIFTMOD                         => 0xB7, 0xA9, 0x20
        QADDRSHIFTMODR                        => 0xB7, 0xA9, 0x21
        QADDRSHIFTMODC                        => 0xB7, 0xA9, 0x22

        MULADDRSHIFTMOD                       => 0xA9, 0xA0
        MULADDRSHIFTRMOD                      => 0xA9, 0xA1
        MULADDRSHIFTCMOD                      => 0xA9, 0xA2
        QMULADDRSHIFTMOD                      => 0xB7, 0xA9, 0xA0
        QMULADDRSHIFTRMOD                     => 0xB7, 0xA9, 0xA1
        QMULADDRSHIFTCMOD                     => 0xB7, 0xA9, 0xA2

        LSHIFTADDDIVMOD                       => 0xA9, 0xC0
        LSHIFTADDDIVMODR                      => 0xA9, 0xC1
        LSHIFTADDDIVMODC                      => 0xA9, 0xC2
        QLSHIFTADDDIVMOD                      => 0xB7, 0xA9, 0xC0
        QLSHIFTADDDIVMODR                     => 0xB7, 0xA9, 0xC1
        QLSHIFTADDDIVMODC                     => 0xB7, 0xA9, 0xC2
    }

    fn add_commands<'a>(
        &mut self,
        iter: impl IntoIterator<Item = &'a (&'static str, CompileHandler)>,
    ) {
        // Add automatic commands
        for (command, handler) in iter {
            if self.handlers.insert(command, *handler).is_some() {
                panic!("Token {} was already registered.", command);
            }
        }
    }

    /// Add automatic commands
    pub fn add_simple_commands(&mut self) {
        self.add_commands(Self::enumerate_simple_commands());

        self.add_commands(Self::enumerate_ton_commands());
    }
}
