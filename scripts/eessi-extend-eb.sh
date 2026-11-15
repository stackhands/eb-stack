#!/bin/bash
# Load EESSI-extend and exec EasyBuild. Runs inside eessi_container.sh.
# EESSI init is not nounset-safe.
set -eo pipefail

# EESSI_CVMFS_REPO names a mirror or another EESSI repository.
repo=${EESSI_CVMFS_REPO:-/cvmfs/software.eessi.io}
ver=${EESSI_VERSION:-2026.06}
init=${repo}/versions/${ver}/init/lmod/bash
if [[ ! -e $init ]]; then
  echo "eessi-extend-eb: missing ${init}" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$init"

# Exactly one prefix variable; the module reads them at load time.
n=0
[[ -n ${EESSI_USER_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_PROJECT_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_SITE_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_CVMFS_INSTALL:-} ]] && n=$((n + 1))
if [[ $n -gt 1 ]]; then
  echo "eessi-extend-eb: set exactly one of EESSI_USER_INSTALL, EESSI_PROJECT_INSTALL, EESSI_SITE_INSTALL, EESSI_CVMFS_INSTALL" >&2
  exit 3
fi
# EESSI-extend refuses a user prefix that does not exist. Project and
# site prefixes need their group and modes set by an owner, so they are
# not created here.
if [[ $n -eq 0 ]]; then
  export EESSI_USER_INSTALL=$HOME/EESSI
fi
if [[ -n ${EESSI_USER_INSTALL:-} ]]; then
  mkdir -p "$EESSI_USER_INSTALL"
fi

export WORKING_DIR=${WORKING_DIR:-/tmp}
# Host Cargo/CMake wrappers are not in the EESSI module graph. Same
# contract as cargo::eessi_cargo_host_isolation (triple and compat arch
# from the build host). CMAKE launchers stay here: they are not cargo.
unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTFLAGS CARGO_ENCODED_RUSTFLAGS \
  CMAKE_C_COMPILER_LAUNCHER CMAKE_CXX_COMPILER_LAUNCHER CMAKE_Fortran_COMPILER_LAUNCHER || true
eval "$(env | awk -F= '/^CARGO_TARGET_[A-Z0-9_]*_(LINKER|RUSTFLAGS)=/{print "unset "$1}')"
export LINKER=${CC:-gcc}
_triple=$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')
_triple_env=$(printf %s "${_triple:-}" | tr '[:lower:]-' '[:upper:]_')
if [[ -n ${_triple_env} ]]; then
  eval "export CARGO_TARGET_${_triple_env}_LINKER=\${CC:-gcc}"
fi
# foss wrappers can drop the compat-layer ld that collect2 needs. The
# init above sets EESSI_EPREFIX to the compat layer it loaded.
_arch=$(uname -m)
export PATH="${EESSI_EPREFIX:-${repo}/versions/${ver}/compat/linux/${_arch}}/usr/bin:${PATH}"
module load EESSI-extend
exec eb "$@"
