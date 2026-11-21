"""Easyblock whose make_module_step records a version the recipe does not."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_HookVersion(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        self.cfg['version'] = self.cfg['version'] + '.post1'
        return super().make_module_step(*args, **kwargs)
