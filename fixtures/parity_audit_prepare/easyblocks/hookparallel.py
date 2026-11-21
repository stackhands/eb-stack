"""Easyblock whose module records the build parallelism."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_HookParallel(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        self.cfg['modextravars'] = {'HOOKPARALLEL_JOBS': str(self.cfg.parallel)}
        return super().make_module_step(*args, **kwargs)
