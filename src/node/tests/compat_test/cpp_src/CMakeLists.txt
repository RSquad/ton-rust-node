cmake_minimum_required(VERSION 3.16)
project(compat_test_node)

# Require paths to be set
if(NOT DEFINED TON_SRC_PATH)
    message(FATAL_ERROR "TON_SRC_PATH must be defined. Pass -DTON_SRC_PATH=/path/to/ton-cpp-testnet")
endif()

if(NOT DEFINED TON_BUILD_PATH)
    # Default to build directory inside source
    set(TON_BUILD_PATH "${TON_SRC_PATH}/build")
endif()

if(NOT EXISTS "${TON_SRC_PATH}/adnl")
    message(FATAL_ERROR "TON_SRC_PATH (${TON_SRC_PATH}) does not appear to be a valid TON source directory")
endif()

if(NOT EXISTS "${TON_BUILD_PATH}")
    message(FATAL_ERROR "TON_BUILD_PATH (${TON_BUILD_PATH}) does not exist. Please build TON first:\n"
                        "  cd ${TON_SRC_PATH}\n"
                        "  mkdir build && cd build\n"
                        "  cmake ..\n"
                        "  cmake --build . --target overlay adnl adnltest dht tl_api keys keyring fec rldp rldp2 tdutils tdactor tdnet ton_crypto")
endif()

message(STATUS "Using TON source: ${TON_SRC_PATH}")
message(STATUS "Using TON build: ${TON_BUILD_PATH}")

# Set C++ standard (TON requires C++20 for 'requires' expressions)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Find required packages (same as TON uses)
find_package(OpenSSL REQUIRED)
find_package(ZLIB REQUIRED)
find_package(Threads REQUIRED)

# Our test binary
add_executable(compat_test_node
    compat_test_node.cpp
)

# Include directories from TON source
target_include_directories(compat_test_node PRIVATE
    ${TON_SRC_PATH}
    ${TON_SRC_PATH}/tdutils
    ${TON_SRC_PATH}/tdactor
    ${TON_SRC_PATH}/tdnet
    ${TON_SRC_PATH}/crypto
    ${TON_SRC_PATH}/tl
    ${TON_SRC_PATH}/tl-utils
    ${TON_SRC_PATH}/keys
    ${TON_SRC_PATH}/keyring
    ${TON_SRC_PATH}/adnl
    ${TON_SRC_PATH}/overlay
    ${TON_SRC_PATH}/rldp
    ${TON_SRC_PATH}/rldp2
    ${TON_SRC_PATH}/fec
    ${TON_SRC_PATH}/quic
    ${TON_SRC_PATH}/metrics
    ${TON_SRC_PATH}/tddb
    ${TON_SRC_PATH}/third-party/abseil-cpp
    ${TON_SRC_PATH}/third-party/ngtcp2/lib/includes
    ${TON_SRC_PATH}/third-party/ngtcp2/crypto/includes
    ${TON_BUILD_PATH}/third-party/ngtcp2/lib/includes
    ${TON_SRC_PATH}/tl/generate  # For auto-generated TL headers
    ${TON_BUILD_PATH}
    ${TON_BUILD_PATH}/tdutils
)

# Find libraries in TON build directory
set(TON_LIB_DIRS
    ${TON_BUILD_PATH}/overlay
    ${TON_BUILD_PATH}/adnl
    ${TON_BUILD_PATH}/dht
    ${TON_BUILD_PATH}/rldp
    ${TON_BUILD_PATH}/rldp2
    ${TON_BUILD_PATH}/fec
    ${TON_BUILD_PATH}/tl
    ${TON_BUILD_PATH}/tl-utils
    ${TON_BUILD_PATH}/keys
    ${TON_BUILD_PATH}/keyring
    ${TON_BUILD_PATH}/tdutils
    ${TON_BUILD_PATH}/tdactor
    ${TON_BUILD_PATH}/tdnet
    ${TON_BUILD_PATH}/tddb
    ${TON_BUILD_PATH}/tddb/td/db
    ${TON_BUILD_PATH}/tdfec
    ${TON_BUILD_PATH}/tdfec/td/fec
    ${TON_BUILD_PATH}/crypto
    ${TON_BUILD_PATH}/common
    ${TON_BUILD_PATH}/quic
    ${TON_BUILD_PATH}/metrics
    ${TON_BUILD_PATH}/third-party/crc32c
    ${TON_BUILD_PATH}/third-party/libraptorq
    ${TON_BUILD_PATH}/third-party/rocksdb
    ${TON_BUILD_PATH}/third-party/ngtcp2/lib
    ${TON_BUILD_PATH}/third-party/ngtcp2/crypto/ossl
)

# Function to find a library in TON build dirs
function(find_ton_library VAR_NAME LIB_NAME)
    find_library(${VAR_NAME}
        NAMES ${LIB_NAME}
        PATHS ${TON_LIB_DIRS}
        PATH_SUFFIXES Release Debug RelWithDebInfo
        NO_DEFAULT_PATH
    )
    if(NOT ${VAR_NAME})
        message(FATAL_ERROR "Could not find TON library: ${LIB_NAME}\n"
                           "Make sure you have built TON with: cmake --build . --target ${LIB_NAME}")
    endif()
    message(STATUS "Found ${LIB_NAME}: ${${VAR_NAME}}")
endfunction()

# Find TON libraries
find_ton_library(TON_OVERLAY overlay)
find_ton_library(TON_ADNL adnl)
find_ton_library(TON_DHT dht)
find_ton_library(TON_RLDP rldp)
find_ton_library(TON_RLDP2 rldp2)
find_ton_library(TON_FEC fec)
find_ton_library(TON_TL_API tl_api)
find_ton_library(TON_TL_UTILS tl-utils)
find_ton_library(TON_KEYS keys)
find_ton_library(TON_KEYRING keyring)
find_ton_library(TON_TDUTILS tdutils)
find_ton_library(TON_TDACTOR tdactor)
find_ton_library(TON_TDNET tdnet)
find_ton_library(TON_TDDB tddb)
find_ton_library(TON_TDFEC tdfec)
find_ton_library(TON_CRYPTO ton_crypto)
find_ton_library(TON_CRYPTO_CORE ton_crypto_core)
find_ton_library(TON_COMMON common)
find_ton_library(TON_TON_BLOCK ton_block)
find_ton_library(TON_CRC32C crc32c)
find_ton_library(TON_ROCKSDB rocksdb)
find_ton_library(TON_QUIC quic)
find_ton_library(TON_METRICS metrics)

# ngtcp2 libraries (required by quic)
find_library(NGTCP2_LIB
    NAMES ngtcp2
    PATHS ${TON_BUILD_PATH}/third-party/ngtcp2/lib
    NO_DEFAULT_PATH
)
find_library(NGTCP2_CRYPTO_OSSL_LIB
    NAMES ngtcp2_crypto_ossl
    PATHS ${TON_BUILD_PATH}/third-party/ngtcp2/crypto/ossl
    NO_DEFAULT_PATH
)

# Optional: RaptorQ library (for FEC)
find_library(RAPTORQ_LIB
    NAMES RaptorQ
    PATHS ${TON_BUILD_PATH}/third-party/libraptorq
    NO_DEFAULT_PATH
)

target_link_libraries(compat_test_node PRIVATE
    ${TON_QUIC}
    ${TON_METRICS}
    ${TON_OVERLAY}
    ${TON_ADNL}
    ${TON_DHT}
    ${TON_RLDP}
    ${TON_RLDP2}
    ${TON_FEC}
    ${TON_TDFEC}
    ${TON_TL_API}
    ${TON_TL_UTILS}
    ${TON_KEYS}
    ${TON_KEYRING}
    ${TON_COMMON}
    ${TON_TON_BLOCK}
    ${TON_TDDB}
    ${TON_TDUTILS}
    ${TON_TDACTOR}
    ${TON_TDNET}
    ${TON_CRYPTO}
    ${TON_CRYPTO_CORE}
    ${TON_CRC32C}
    ${TON_ROCKSDB}
    OpenSSL::Crypto
    OpenSSL::SSL
    ZLIB::ZLIB
    Threads::Threads
)

# Add ngtcp2 if found (required by quic)
if(NGTCP2_LIB)
    target_link_libraries(compat_test_node PRIVATE ${NGTCP2_LIB})
endif()
if(NGTCP2_CRYPTO_OSSL_LIB)
    target_link_libraries(compat_test_node PRIVATE ${NGTCP2_CRYPTO_OSSL_LIB})
endif()

# Add RaptorQ if found
if(RAPTORQ_LIB)
    target_link_libraries(compat_test_node PRIVATE ${RAPTORQ_LIB})
endif()

# Platform-specific settings
if(APPLE)
    target_link_libraries(compat_test_node PRIVATE
        "-framework Security"
        "-framework CoreFoundation"
        "-framework CoreServices"
    )
    target_link_libraries(compat_test_node PRIVATE c++)
elseif(UNIX)
    target_link_libraries(compat_test_node PRIVATE dl)
endif()

# Add additional libraries that TON depends on
find_library(LZ4_LIBRARY lz4)
if(LZ4_LIBRARY)
    target_link_libraries(compat_test_node PRIVATE ${LZ4_LIBRARY})
endif()

find_library(SODIUM_LIBRARY sodium)
if(SODIUM_LIBRARY)
    target_link_libraries(compat_test_node PRIVATE ${SODIUM_LIBRARY})
endif()

# blst (BLS crypto, required by ton_crypto)
find_library(BLST_LIBRARY
    NAMES blst
    PATHS ${TON_BUILD_PATH}/third-party/blst
    NO_DEFAULT_PATH
)
if(BLST_LIBRARY)
    target_link_libraries(compat_test_node PRIVATE ${BLST_LIBRARY})
endif()

# secp256k1 (required by ton_crypto_core)
find_library(SECP256K1_LIBRARY
    NAMES secp256k1
    PATHS ${TON_BUILD_PATH}/third-party/secp256k1/lib
    NO_DEFAULT_PATH
)
if(SECP256K1_LIBRARY)
    target_link_libraries(compat_test_node PRIVATE ${SECP256K1_LIBRARY})
endif()

# Abseil libraries (required by ton_block hash containers)
file(GLOB_RECURSE ABSEIL_LIBS "${TON_BUILD_PATH}/third-party/abseil-cpp/absl/*.a")
if(ABSEIL_LIBS)
    target_link_libraries(compat_test_node PRIVATE ${ABSEIL_LIBS})
endif()

# macOS: re-sign binary to avoid dyld hang caused by com.apple.provenance xattr
if(APPLE)
    add_custom_command(TARGET compat_test_node POST_BUILD
        COMMAND codesign --force --sign - $<TARGET_FILE:compat_test_node>
        COMMENT "Re-signing binary for macOS"
    )
endif()
