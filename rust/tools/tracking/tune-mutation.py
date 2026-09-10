from pathlib import Path
import subprocess
p=Path('/home/lilith/work/zen/zenav1-svt/rust/crates/svtav1-encoder/src/pipeline.rs')
s=p.read_text()
fixed='''            if !is_key
                && matches!(
                    self.hdr.tune,
                    crate::tune::TUNE_VQ | crate::tune::TUNE_FILM_GRAIN
                )
            {
                base
            } else {
                crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
            }
'''
old='''            if self.hdr.is_fork() {
                crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
            } else {
                base
            }
'''
assert s.count(fixed)==1
try:
 p.write_text(s.replace(fixed,old))
 subprocess.run(['python3','/home/lilith/tmp/svt-tracking/tune-probe.py'],check=True)
 d=Path('/home/lilith/tmp/svt-tracking/tune-before')
 assert (d/'tune0.obu').read_bytes() != (d/'tune0.c.obu').read_bytes()
 assert (d/'tune1.obu').read_bytes() == (d/'tune1.c.obu').read_bytes()
finally:p.write_text(s)
subprocess.run(['python3','/home/lilith/tmp/svt-tracking/tune-probe-after.py'],check=True)
d=Path('/home/lilith/tmp/svt-tracking/tune-after')
for tune in (0,1):assert (d/f'tune{tune}.obu').read_bytes()==(d/f'tune{tune}.c.obu').read_bytes()
