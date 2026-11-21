"""Easyblock whose class name EasyBuild encodes, as upstream's FFTW.MPI one."""
from easybuild.easyblocks.generic.configuremake import ConfigureMake


class EB_FFTW_period_MPI(ConfigureMake):
    def make_module_step(self, *args, **kwargs):
        self.cfg['modextravars'] = {'FFTW_MPI_DECODED': 'yes'}
        return super().make_module_step(*args, **kwargs)
