# Review queue

Steps that need the project owner: tests on the owner's PC and interface,
listening, and decisions to accept. Work carries on without them, at risk,
and every new item is added as it comes up. Tick items off as they are
done, and record results in the linked documents.

Test machine: Windows PC with a Behringer UMC1820 interface.

## One file per feature

Each feature, spike or review gate has its own file in this folder, named
after its roadmap item, such as `1.2-recording.md` or `0.6-speed-pitch.md`.
The folder listing is the index, in roadmap order, so this README does not
list the files and needs no edit when one is added.

Each file holds that feature's checks on the PC, listening and decisions
to accept, under headings where it has more than one kind.

To see everything still open, from the repository root:

```
git grep -n -- "- \[ \]" docs/review-queue
```

### Adding items

- New work gets a **new file** here, in the same commit as the work. Copy
  the shape of an existing one: a `#` heading with the roadmap number and
  name, then `- [ ]` items saying what to do and what counts as a pass.
- Add to an existing file only when the work belongs to that feature.
  Don't edit other features' files or this README in passing: that is what
  made the old single list conflict between pull requests.
- When a feature has two parts reviewed separately, use two files with the
  same number, such as `1.6-comments.md` and `1.6-identity.md`.

## Getting the test programs

1. Open the repository's **Actions** tab, then the latest green **spikes**
   run on `main`.
2. Under **Artifacts**, download `spikes-windows-x64` (the Windows
   programs) and `stretch-listening` (audio files and report).
3. Unzip into a folder and open a terminal there. Each program writes a
   `*-report.txt` file next to itself. Commit the reports, or paste them
   into a chat thread.

The app itself, `daw.exe`, is in the **daw-windows-x64** artifact of the
latest **ci** run on `main` (not the spikes run, and not a pull request:
pull requests no longer build it). Screenshots named in the files are
in the project files under `/mnt/project-files/screenshots/`.
