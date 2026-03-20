#!/bin/sh

# Disable Intel Turbo Boost
echo 1 >/sys/devices/system/cpu/intel_pstate/no_turbo

# Set performance governor
for governor in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  echo performance >$governor
done

# Disable Hyper-threading
echo off >/sys/devices/system/cpu/smt/control

# Disable ASLR
echo 0 >/proc/sys/kernel/randomize_va_space
