# Research: privacy and children's data (roadmap 1.8)

Checked 2026-10-28. The app is used for music lessons, and some students
will be children. This review looks at which child-data and privacy rules
could apply, and what that means for the identity, live session and
comment work still to come in roadmap 1.6. Items marked **Owner decision**
are also listed in [REVIEW_QUEUE.md](../REVIEW_QUEUE.md).

**This is not legal advice.** It summarises what regulators' own pages
say, and marks clearly where the pages are silent or where I am drawing an
inference.

**Scope (2026-10-28):** the app is built for the owner and their guitar
teacher, and published as open source, but not promoted to schools or
children. No professional legal check is planned. The design implications
below are kept as good practice, and this review should be revisited if
the app is ever offered to schools or children, or if any hosted service
is added.

## Summary

- The rules mostly bite on whoever **runs the service and handles the
  data**. Because projects stay on users' machines and there is no server
  we operate, very little of the data ever reaches this project. That is
  the strongest privacy property we have, and it should be kept on purpose,
  not by accident.
- Data still moves **between users**: recordings of a student's voice,
  comments, lesson notes, display names, keys and network addresses.
  A teacher who teaches for money is probably handling that data as part
  of their work, so the law treats the teacher as responsible for it, not
  the app. The app should make that easy to do well.
- Even where the rules may not strictly apply to us, the UK children's
  code is a good design checklist. Most of its 15 standards already match
  the plan (no analytics, no profiling, no location, local by default).
  The gaps are in the 1.6 work: **deleting data out of edit history**,
  **who can join a live session**, **what identity reveals**, and
  **third parties seeing network addresses**.
- The UK Online Safety Act is the least clear fit. Its definition of a
  user-to-user service is broad, and I could not reach the regulator's
  plain-language explainer. It needs a proper answer before release.

## What personal data the app handles

| Data | Where it lives | Who can see it | Notes |
|---|---|---|---|
| Recorded takes (a student's voice and playing) | Project folder, `Unsaved takes` | Project members, anyone sent a bundle | A recording of a child's voice is listed as personal information under the US rule (see below). |
| Video (Phase 3 clips, Phase 4 lesson calls) | Project folder; calls are live only | Members; the other end of a call | Image and voice of the student. |
| Lesson notes, pinned comments | The project document | Members | Free text: may hold names, ages, school, health or anything else typed in. |
| Display name, public key (1.6, planned) | The project document | Members | A key used across projects is a persistent identifier that links one person's projects together. |
| Edit history | The project document | Members, anyone sent a bundle | The full save format keeps history, so text that was deleted can still be recovered from the file. Once identity lands, history also records who made each change. |
| Presence: who is here, playheads | Session channel only, not stored | Session members | Already planned as not stored. |
| Network addresses | Live sessions | The other peer; any address-lookup server used | Addresses are listed as online identifiers in UK guidance. |
| Crash reports (0.7, planned) | Local files | Whoever the user sends them to | May contain file paths with the user's account name. |
| Update check (0.7, planned) | — | The site hosting release files | Sees the machine's address each time it checks. |

## Who is responsible

- **UK GDPR roles.** "A controller determines the purposes and means of
  processing personal data"; a processor processes it on a controller's
  behalf. *(Source: ICO, "Who does the UK GDPR apply to?", read
  2026-10-28. The page notes it is under review after the Data (Use and
  Access) Act.)* The page does not say how this applies to someone who
  writes software that runs entirely on users' own machines and never
  sends data to them. **Inference, not confirmed:** if no personal data
  reaches this project, the project is not processing it, so it is not a
  controller of lesson data. Any server, relay, telemetry or hosted
  feature we add later would change that.
- **The household exclusion.** UK GDPR does not cover "personal data
  processed in the course of a purely personal or household activity" with
  "no connection to a professional or commercial activity". *(Source: ICO,
  "A guide to the data protection exemptions", read 2026-10-28.)* A parent
  recording their own child at home is likely outside the law. **Inference:**
  a paid teacher recording a student has a connection to a professional
  activity, so the teacher is likely a controller of the student's
  recordings, comments and notes. The ICO page gives no example of this
  case.
- **Schools.** Schools, and teachers working for them, will have their own
  data protection duties and policies. The app should not get in their way
  (see the design implications).

## United Kingdom

### UK GDPR and the age of consent

- Only children aged 13 and over can give their own consent to an
  information society service; under 13, a person with parental
  responsibility must give or authorise it, with reasonable efforts to
  verify. This applies when the service is offered directly to children
  **and relies on consent** as its lawful basis. *(Source: ICO, "How do
  the lawful bases apply to children's personal information?", read
  2026-10-28.)*
- If the app collects nothing, there is nothing to consent to on our side.
  A teacher may still need a lawful basis for recording a student. That is
  the teacher's choice, not something the app can decide for them.

### Age appropriate design code (the "children's code")

- A statutory code under section 123 of the Data Protection Act 2018. Not
  following it makes it "more difficult to demonstrate that your
  processing is fair"; the ICO can act on breaches of UK GDPR. *(Source:
  ICO, "About this code", read 2026-10-28.)*
- It covers **information society services** "likely to be accessed by
  children" (under 18), where "the possibility of this happening needs to
  be more probable than not". Most online services are information
  society services, "including apps, programs and many websites", and it
  also covers "not-for-profit apps, games and educational sites". The
  relevant services are "those which involve the processing of personal
  data to which the GDPR applies". The page does not discuss software that
  runs offline or keeps all data on the device. *(Source: ICO, "Services
  covered by this code", read 2026-10-28.)*
- **Fit for us:** music lessons for children make it likely that children
  use the app. Whether the code formally applies to this project depends on
  whether we process personal data at all (see "Who is responsible"). We
  should design to it either way.

The 15 standards, and where we stand *(source: ICO, "Code standards", read
2026-10-28)*:

| # | Standard | Status in our plan |
|---|---|---|
| 1 | Best interests of the child | Use this review as the record. |
| 2 | Data protection impact assessment | Write one if we ever run a service that touches personal data. |
| 3 | Age appropriate application | Apply child-safe defaults to **everyone**, rather than asking ages. The code allows applying it to all users. |
| 4 | Transparency | **Gap:** a short, plain-language privacy note in the app, readable by a child, shown before the first share or live session. |
| 5 | Detrimental use of data | Nothing planned that uses data against users. |
| 6 | Policies and community standards | Only what we publish; keep it short and true. |
| 7 | Default settings | **Gap:** anything that reaches a third party (address lookup, update checks) should be off or asked about first. |
| 8 | Data minimisation | Display name only; no email, age, photo or contacts. **Gap:** bundles carry all takes and history. |
| 9 | Data sharing | No sharing with us. **Gap:** address-lookup servers see addresses. |
| 10 | Geolocation | None collected. Keep it that way. |
| 11 | Parental controls | None planned. If a teacher can watch a student's screen (follow mode, presence), show the student clearly that they are being watched. |
| 12 | Profiling | None. Any future practice statistics stay local and are never used to rank or target. |
| 13 | Nudge techniques | No prompts that push sharing. Sharing and joining are always the user's explicit action. |
| 14 | Connected toys and devices | Not applicable (the phone remote in Phase 5 stays on the local network). |
| 15 | Online tools | **Gap:** tools to see, export and delete one person's data from a project. |

### Online Safety Act 2023

- A **user-to-user service** is "an internet service by means of which
  content that is generated directly on the service by a user of the
  service, or uploaded to or shared on the service by a user of the
  service, may be encountered by another user, or other users, of the
  service". It is enough that the service "has a functionality that allows
  such sharing". *(Source: Online Safety Act 2023, section 3, on the UK
  legislation website, read 2026-10-28.)* An "internet service" is "a
  service that is made available by means of the internet" (section 228).
- A service is in scope if it has links with the UK, for example "a
  significant number of United Kingdom users" or the UK as a target market
  (section 4).
- The **provider** is "the entity that has control over who can use the
  user-to-user part of the service"; if no entity has that control but
  individuals do, those individuals are treated as the provider (section
  226(2)–(3)).
- Exemptions include services whose only user content is email, SMS/MMS,
  or "one-to-one live aural communications"; "limited functionality"
  services where users can only comment on or rate the **provider's own
  content**; and services provided by people responsible for education or
  childcare, subject to safeguarding duties, for that education
  (Schedule 1, paragraphs 1–4 and 10). Paragraph 6 limits some of these.
- **Fit for us — open question.** Live sessions, comments and shared
  takes let one user's content reach another over the internet. Whether a
  peer-to-peer app with no server is a "service made available by means of
  the internet", and whether its provider would be this project or the
  teacher who controls who joins, is not answered by the text I read. The
  limited functionality exemption does not seem to fit, because comments
  are on users' own projects, not on our content. The education exemption
  might cover a school's own use but not the app itself.
- **Could not reach:** the regulator's "Online Safety Act explained" Q&A
  (PDF) failed to load, and its online regulation checker is interactive,
  so I did not use it. Both should be checked before release.

## United States: COPPA

- Applies to "operators of commercial websites and online services
  (including mobile apps and IoT devices, such as smart toys)" directed to
  children under 13, or with actual knowledge that they collect personal
  information from children under 13. An online service includes "any
  service available over the Internet, or that connects to the Internet or
  a wide-area network". Foreign operators involved in commerce are
  included. *(Source: FTC, "Complying with COPPA: Frequently Asked
  Questions", sections A.1, A.2, A.7, B.7; page dated 2026-07-10, read
  2026-10-28.)*
- Personal information includes a "persistent identifier that can be used
  to recognize a user over time" and "a photograph, video, or audio file,
  where such file contains a child's image or voice" (A.3).
- **Data that stays on the device:** "You are not collecting personal
  information simply because your app interacts with personal
  information" that "is stored on the device and is never transmitted"
  (F.5). This supports the local-first design.
- But "you are responsible for the collection of personal information from
  your users, no matter who is doing the collection", and operators are
  liable for collection "that occurs on or through your sites and services"
  (D.9, D.10). **Inference:** if the app sends a child's recording to a
  teacher over a live session, that is data leaving the device through our
  software. Whether that counts as *our* collection when it never reaches
  us is not answered on the FAQ page.
- Letting children make personal information public (forums, profiles)
  counts as collection and needs a stronger consent method (A.10, D.12,
  I.4). We should never add a public space.
- Schools can consent for parents, but only "limited to the educational
  context" (N.1).
- The rule was amended on 22 April 2025. "The amended Rule is effective
  June 23, 2025", and most of it had to be complied with by 22 April
  2026. *(Source: the Federal Register notice of 22 April 2025, read
  2026-10-28.)* I did not read the amendment's detailed security and
  retention requirements; the summary I fetched cut off before them.
- **Unknown:** the FAQ speaks of "commercial" websites and services. I did
  not find what that means for a free, open-source project with no
  revenue. Not checked further.

## European Union: GDPR

- Where consent is the basis, a child's own consent is valid from an age
  that "varies between 13 and 16 years, depending on the age established
  in each EU Member State"; below it, a parent or guardian must consent.
  Information for children must be in "clear and plain language that a
  child can easily understand". *(Source: European Commission, "Are there
  any specific safeguards for data about children?", read 2026-10-28.)*
- The EU's guidance on data protection by design could not be fetched
  (server error), so it is not summarised here.

## Not reviewed

Other countries' rules (for example Australia, Canada, and US state laws)
were not checked. Equality and safeguarding duties that apply to teachers
and schools are outside this review.

## Design implications

These are for the 1.6 identity, live session and comment work, and for
0.7 distribution. They are proposals; the ones that change the plan are
owner decisions.

### Keep data away from us (architecture rule)

1. **No personal data reaches anything this project runs.** No accounts,
   no telemetry, no hosted relay, directory or comment store. If a relay or
   any hosted feature is ever proposed, it needs a fresh privacy review
   first, because it would likely make the project a controller (UK/EU)
   and an operator (US). **Owner decision:** adopt this as a written rule.
2. **Third parties that see addresses are opt-in.** The address-lookup
   servers for live sessions (already in the review queue) and the update
   check both reveal the machine's address to someone else. Ask once, in
   plain words, before the first use, and offer manual addresses. The 0.7
   proposal already makes the update check opt-in, which fits. **Owner
   decision** (joins the existing address-lookup item).

### Identity

3. **Ask for a display name only**, with a hint that a first name or
   nickname is enough. Never ask for email, age, photo, school or contacts.
4. **Keys per project, not per install.** A single key per install lets
   anyone who receives two projects link them to the same child. A fresh
   key for each project (with the owner key still proving the teacher's
   role) avoids that. It costs a little: a teacher can't recognise a
   student across projects by key alone, only by name. **Owner decision.**
5. **No age gate.** Asking ages is collecting data. Apply child-safe
   defaults to everyone instead (children's code standard 3 allows this).

### Live sessions

6. **The host admits each person.** An invite code alone should not let
   someone in: the host sees the joiner's display name and accepts or
   refuses. Codes expire and are single-use by default.
7. **No discovery.** Sessions are never listed or searchable; joining is
   only by a code the host shares.
8. **Make watching visible.** When follow mode or presence is on, the
   student sees who is in the session and that the teacher can see their
   playhead. When audio or video is being sent (Phase 4 calls), show it
   clearly, every time. Calls are off until each person turns them on.
9. **Students can't contact each other** through the app unless the owner
   adds them to the same project.
10. **Nothing from a session is recorded** unless a member presses record,
    and everyone in the session sees that recording has started.

### Comments, notes and history

11. **Real deletion.** Deleting a comment, note or take must remove it from
    the saved file, not just hide it. The full save format keeps edit
    history, so this needs a way to save without the old history (or to
    compact it), and bundles must be built the same way. This is the
    biggest gap for the right to erasure. Note it in decision 0004's
    follow-up work.
12. **Per-person data tools.** The owner can list, export and delete
    everything one member contributed to a project (takes, comments, notes,
    their name in history). A member can see the same list for themselves.
13. **Show what a bundle contains** before it is sent: number of takes,
    whether unused takes and history are included, with options to leave
    them out.
14. **Comments only between members.** No public links, no posting outside
    the project.

### Distribution and diagnostics

15. **Crash reports**: remove user names and file paths, and show the user
    the exact text before they choose to send it.
16. **Plain-language privacy note** in the app and the repository, written
    so a child can follow it: what is stored, where, who can see it, and how
    to delete it. Shown before the first share or live session.
17. **A short note for teachers**: that they may be responsible for their
    students' recordings and notes, that parents should know lessons are
    recorded, and how to delete a student's data. Plain facts and links to
    the regulator, not legal advice.

## Sources

All read 2026-10-28 unless marked.

- ICO, Services covered by this code:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/childrens-information/childrens-code-guidance-and-resources/age-appropriate-design-a-code-of-practice-for-online-services/services-covered-by-this-code/
- ICO, Code standards:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/childrens-information/childrens-code-guidance-and-resources/age-appropriate-design-a-code-of-practice-for-online-services/code-standards/
- ICO, About this code:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/childrens-information/childrens-code-guidance-and-resources/age-appropriate-design-a-code-of-practice-for-online-services/about-this-code/
- ICO, How do the lawful bases apply to children's personal information?:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/childrens-information/children-and-the-uk-gdpr/how-do-the-lawful-bases-apply-to-children-s-personal-information
- ICO, Who does the UK GDPR apply to?:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/personal-information-what-is-it/who-does-the-uk-gdpr-apply-to/
- ICO, A guide to the data protection exemptions:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/exemptions/a-guide-to-the-data-protection-exemptions/
- ICO, What are identifiers and related factors?:
  https://ico.org.uk/for-organisations/uk-gdpr-guidance-and-resources/personal-information-what-is-it/what-is-personal-data/what-are-identifiers-and-related-factors/
- Online Safety Act 2023, sections 3, 4, 226, 228 and Schedule 1:
  https://www.legislation.gov.uk/ukpga/2023/50
- FTC, Complying with COPPA: Frequently Asked Questions:
  https://www.ftc.gov/business-guidance/resources/complying-coppa-frequently-asked-questions
- Federal Register, Children's Online Privacy Protection Rule (22 April 2025):
  https://www.federalregister.gov/documents/2025/04/22/2025-05904/childrens-online-privacy-protection-rule
- European Commission, Are there any specific safeguards for data about children?:
  https://commission.europa.eu/law/law-topic/data-protection/information-business-and-organisations/legal-grounds-processing-data/are-there-any-specific-safeguards-data-about-children_en

**Could not be reached:** Ofcom's "Online Safety Act explained" Q&A PDF
(client error), and the EDPB guidelines on data protection by design and by
default (server error).
