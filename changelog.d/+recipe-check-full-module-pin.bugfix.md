Fix `recipe check` rejecting a full toolchain-suffixed module version such as `5.3.0-GCCcore-13.3.0`. `build_order` already accepted it, and SYSTEM recipes like sentieon-genomics depend on it.
