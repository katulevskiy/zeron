#!/usr/bin/env python3
"""Run as root on Exnomic after the dedicated loopback service is healthy."""
from pathlib import Path
import datetime,subprocess,urllib.request
assert b'"ok":true' in urllib.request.urlopen('http://127.0.0.1:3080/health',timeout=10).read()
config=Path('/etc/nginx/nginx.conf');original=config.read_text()
route='\t\tzeron.exnomic.com 127.0.0.1:8573; # Contribution Manager (isolated)\n'
marker='map $ssl_preread_server_name $exnomic_be {\n'
if 'zeron.exnomic.com' not in original:
 assert original.count(marker)==1,'Unexpected SNI router; refusing to change it'
 backup=Path('/opt/contribution-manager/deployment-backup')/('nginx-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.conf')
 backup.write_text(original)
 config.write_text(original.replace(marker,marker+route,1))
link=Path('/etc/nginx/sites-enabled/contribution-manager.conf');target=Path('/etc/nginx/sites-available/contribution-manager.conf')
if link.is_symlink():assert link.resolve()==target
elif link.exists():raise RuntimeError('Refusing to replace an unrelated site file')
else:link.symlink_to(target)
result=subprocess.run(['nginx','-t'],capture_output=True,text=True)
if result.returncode:
 config.write_text(original);link.unlink();raise RuntimeError(result.stderr)
subprocess.run(['systemctl','reload','nginx'],check=True)
print('Added one isolated SNI route and site; Nginx config passed and gracefully reloaded.')
