"""Exercise release promotion without contacting GitHub or publishing anything."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("publish.sh").resolve()
MOCK_GH = '''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['MOCK_CALLS'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
if args[:2] == ['release', 'upload']:
    sys.exit(1 if os.environ['SCENARIO'] == 'upload-failure' else 0)
if '--method' in args:
    sys.exit(0)
if args[1].endswith('/latest'):
    print('123')
else:
    print(pathlib.Path(os.environ['MOCK_RELEASE']).read_text())
'''


class PublishTests(unittest.TestCase):
    def run_scenario(self, scenario):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dist = root / 'dist'
            dist.mkdir()
            files = {'Alacritty-macos-universal.zip': b'zip',
                     'Alacritty-macos-universal.dmg': b'dmg',
                     'appcast.xml': b'<rss>signed feed fixture</rss>'}
            files['SHA256SUMS-macos.txt'] = ''.join(
                f'{hashlib.sha256(data).hexdigest()}  {name}\n'
                for name, data in files.items()
            ).encode()
            for name, data in files.items():
                (dist / name).write_bytes(data)
            assets = [{'name': name, 'size': len(data), 'state': 'uploaded',
                       'digest': 'sha256:' + hashlib.sha256(data).hexdigest()}
                      for name, data in files.items()]
            if scenario == 'digest-mismatch':
                assets[0]['digest'] = 'sha256:bad'
            if scenario == 'bad-checksum':
                (dist / 'Alacritty-macos-universal.zip').write_bytes(b'corrupt')
            if scenario == 'missing-appcast':
                (dist / 'appcast.xml').unlink()
            release = root / 'release.json'
            release.write_text(json.dumps({'tag_name': 'v1.0.0', 'draft': False,
                                          'prerelease': scenario != 'stable',
                                          'assets': assets}))
            mock = root / 'gh'
            mock.write_text(MOCK_GH)
            mock.chmod(0o700)
            calls_file = root / 'calls'
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                       GH_TOKEN='test', GITHUB_REPOSITORY='owner/repo',
                       RELEASE_ID='123', RELEASE_TAG='v1.0.0', SCENARIO=scenario,
                       MOCK_CALLS=str(calls_file), MOCK_RELEASE=str(release))
            result = subprocess.run(['bash', str(SCRIPT)], cwd=root, env=env,
                                    capture_output=True, text=True)
            calls = [json.loads(line) for line in calls_file.read_text().splitlines()] if calls_file.exists() else []
            promotions = [args for args in calls if '--method' in args]
            uploads = [args for args in calls if args[:2] == ['release', 'upload']]
            if scenario == 'success':
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(promotions), 1)
                self.assertIn('prerelease=false', promotions[0])
                self.assertIn('make_latest=true', promotions[0])
                self.assertLess(calls.index(uploads[0]), calls.index(promotions[0]))
            else:
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(promotions, [])
                if scenario in ('bad-checksum', 'stable', 'missing-appcast'):
                    self.assertEqual(uploads, [])

    def test_promotion_gates(self):
        for scenario in ('success', 'upload-failure', 'digest-mismatch', 'bad-checksum', 'stable', 'missing-appcast'):
            with self.subTest(scenario=scenario):
                self.run_scenario(scenario)


if __name__ == '__main__':
    unittest.main()
