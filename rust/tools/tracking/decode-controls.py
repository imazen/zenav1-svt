from pathlib import Path
import subprocess
root=Path('/home/lilith/tmp/svt-tracking');n=0
for arm,depth in [('tune-after',8),('scm-after',8),('scm-native10',10)]:
 for p in sorted((root/arm).glob('*.obu')):
  with p.with_suffix('.aom.log').open('w') as f:subprocess.run(['/usr/bin/aomdec','--rawvideo',f'--output-bit-depth={depth}','-o',str(p)+'.decoded',str(p)],stdout=f,stderr=subprocess.STDOUT,check=True)
  assert Path(str(p)+'.decoded').stat().st_size==72*88*3//2*(1 if depth==8 else 2)
  n+=1
print(f'{n} streams decoded with exact expected plane length')
