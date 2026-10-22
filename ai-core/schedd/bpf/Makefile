# Builds the scheduler telemetry object.
#
# Needs clang, bpftool and the libbpf headers (Debian/Ubuntu: clang libbpf-dev
# bpftool; Gentoo: llvm-core/clang dev-libs/libbpf dev-util/bpftool), and a running
# kernel that exposes its types in /sys/kernel/btf/vmlinux. The kernel types are
# dumped into vmlinux.h on the build machine; the object is CO-RE, so it then runs on
# other kernels too.

CLANG ?= clang
BPFTOOL ?= bpftool
VMLINUX_BTF ?= /sys/kernel/btf/vmlinux
ARCH := $(shell uname -m | sed 's/x86_64/x86/; s/aarch64/arm64/')

all: telemetry.bpf.o

vmlinux.h:
	$(BPFTOOL) btf dump file $(VMLINUX_BTF) format c > $@

telemetry.bpf.o: telemetry.bpf.c vmlinux.h
	$(CLANG) -g -O2 -target bpf -D__TARGET_ARCH_$(ARCH) -I. -c telemetry.bpf.c -o $@

clean:
	rm -f telemetry.bpf.o vmlinux.h

.PHONY: all clean
