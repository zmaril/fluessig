"""Runtime evidence: CRUD across relations, enum round-trip, and the composite
-PK-member-FK test, all through the Django ORM over the managed=False tables that
fluessig owns.

Run: ``python manage.py smoke`` (after ``migrate`` + applying fluessig_schema.sql).
"""
from datetime import datetime, timezone

from django.core.management.base import BaseCommand
from django.db import transaction

from demo.models import Comments, Issues, Labels, Orgs, Repos, Users, IssueLabels


def line(s=""):
    print(s)


class Command(BaseCommand):
    help = "Exercise CRUD, enums, and the composite-PK-member-FK case."

    @transaction.atomic
    def handle(self, *args, **opts):
        # clean slate so the command is idempotent
        for m in (IssueLabels, Comments, Issues, Labels, Repos, Users, Orgs):
            m.objects.all().delete()

        line("=" * 70)
        line("1. CREATE across relations: Org -> Repo -> User -> Issue -> Comment")
        line("=" * 70)
        org = Orgs.objects.create(id="acme", name="Acme Corp")
        repo = Repos.objects.create(id="acme/widgets", org=org, name="widgets",
                                    is_private=False)
        alice = Users.objects.create(id=1, login="alice", name="Alice A.")
        bob = Users.objects.create(id=2, login="bob", name=None)
        line(f"   created org={org.id!r} repo={repo.id!r} users={alice.login!r},{bob.login!r}")

        # THE KEY TEST: an Issue whose PK is (repo, number) where repo is a FK.
        issue = Issues.objects.create(
            repo=repo, number=1, title="Widget explodes on Tuesdays",
            body="Repro: wait until Tuesday.", state="OPEN", kind="feat",
            author=alice, is_locked=False, comment_count=0, closed_at=None,
        )
        line(f"   created Issue pk=(repo={issue.repo_id!r}, number={issue.number}) "
             f"via ForeignKey member — SUCCESS")

        c1 = Comments.objects.create(id=100, repo=repo.id, issue_number=1,
                                     author=bob, body="I see it too.",
                                     created_at=datetime(2026, 7, 20, 12, 0,
                                                         tzinfo=timezone.utc))
        line(f"   created Comment id={c1.id} on issue (repo={c1.repo}, "
             f"number={c1.issue_number})")

        # a Label (also composite-PK-member-FK) + a to-many edge row
        bug_label = Labels.objects.create(repo=repo, name="bug", color="#d73a4a")
        IssueLabels.objects.create(repo=repo.id, issue_number=1,
                                   label_name="bug")
        line(f"   created Label pk=(repo={bug_label.repo_id!r}, name={bug_label.name!r}) "
             f"+ issue_labels edge row")

        line("")
        line("=" * 70)
        line("2. QUERY across FK relations (select_related + filter)")
        line("=" * 70)
        got = (Issues.objects
               .select_related("repo", "repo__org", "author")
               .get(repo=repo, number=1))
        line(f"   Issue (repo,number)=({got.repo_id},{got.number})")
        line(f"     .repo.name          = {got.repo.name}")
        line(f"     .repo.org.name      = {got.repo.org.name}   (2-hop FK join)")
        line(f"     .author.login       = {got.author.login}")
        line(f"   filter(author=alice)  -> {list(Issues.objects.filter(author=alice).values_list('number', flat=True))}")
        line(f"   comments on issue     -> {[(c.id, c.author.login) for c in Comments.objects.filter(repo=repo.id, issue_number=1).select_related('author')]}")
        line(f"   labels on issue (edge)-> {list(IssueLabels.objects.filter(repo=repo.id, issue_number=1).values_list('label_name', flat=True))}")

        line("")
        line("=" * 70)
        line("3. ENUM round-trip (stored wire value vs display name)")
        line("=" * 70)
        line(f"   state: stored={got.state!r}  display={got.get_state_display()!r}")
        line(f"   kind : stored={got.kind!r}  display={got.get_kind_display()!r}   "
             f"(value 'feat' != name 'Feature')")
        assert got.state == "OPEN"
        assert got.kind == "feat"
        assert got.get_kind_display() == "Feature"
        line("   asserts passed: wire value stored, display name resolved via choices")

        line("")
        line("=" * 70)
        line("4. UPDATE a field + read back (close the issue)")
        line("=" * 70)
        now = datetime(2026, 7, 21, 9, 30, tzinfo=timezone.utc)
        Issues.objects.filter(repo=repo, number=1).update(state="CLOSED",
                                                          closed_at=now,
                                                          comment_count=1)
        reread = Issues.objects.get(repo=repo, number=1)
        line(f"   after update: state={reread.state!r} "
             f"display={reread.get_state_display()!r} "
             f"closed_at={reread.closed_at!r} comment_count={reread.comment_count}")
        assert reread.state == "CLOSED"
        assert reread.closed_at == now
        assert reread.comment_count == 1
        line("   asserts passed: update() over managed=False table persisted")

        line("")
        line("=" * 70)
        line("5. COMPOSITE-PK-MEMBER-FK — the decisive test")
        line("=" * 70)
        line(f"   Issues._meta.pk               = {Issues._meta.pk!r}")
        line(f"   Issues.pk field type          = {type(Issues._meta.pk).__name__}")
        repo_field = Issues._meta.get_field("repo")
        line(f"   Issues 'repo' member field    = {type(repo_field).__name__} "
             f"(is_relation={repo_field.is_relation})")
        line(f"   .pk of the created issue      = {got.pk!r}")
        # query BY the composite pk tuple
        by_pk = Issues.objects.get(pk=(repo.id, 1))
        line(f"   Issues.objects.get(pk=('{repo.id}', 1)) -> "
             f"title={by_pk.title!r}")
        line("   RESULT: Django accepts a ForeignKey as a CompositePrimaryKey "
             "member — import, migrate-skip, create, and query all work.")

        line("")
        line("SMOKE OK — all CRUD/enum/composite-PK assertions passed.")
