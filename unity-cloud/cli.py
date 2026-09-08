#!/usr/bin/env python3
"""vaked unity-cloud lane — the Unity Cloud Asset Manager, from the constellation.

One door, many lanes: the same SDK surface, scripted for the engine's asset
pipeline. Auth via user_login (browser OAuth PKCE) or service_account
(headless, UNITY_CLOUD_KEY_ID + UNITY_CLOUD_KEY).

Run with uv:  uv run python cli.py <command> [args]
"""

import argparse
import os
import sys

import unity_cloud as uc


def _init(args):
    uc.initialize()
    if args.service_account:
        key_id = os.environ.get("UNITY_CLOUD_KEY_ID")
        key = os.environ.get("UNITY_CLOUD_KEY")
        if not key_id or not key:
            sys.exit("service_account needs UNITY_CLOUD_KEY_ID + UNITY_CLOUD_KEY in env")
        uc.identity.service_account.use(key_id=key_id, key=key)
    else:
        uc.identity.user_login.use()
        state = uc.identity.user_login.get_authentication_state()
        if state != uc.identity.user_login.Authentication_State.LOGGED_IN:
            print("opening the browser for Unity Cloud sign-in...", file=sys.stderr)
            uc.identity.user_login.login()
        if uc.identity.user_login.get_authentication_state() != uc.identity.user_login.Authentication_State.LOGGED_IN:
            sys.exit("not logged in — run 'auth' first or use --service-account")


def cmd_auth(args):
    uc.initialize()
    if args.service_account:
        key_id = os.environ.get("UNITY_CLOUD_KEY_ID")
        key = os.environ.get("UNITY_CLOUD_KEY")
        if not key_id or not key:
            sys.exit("service_account needs UNITY_CLOUD_KEY_ID + UNITY_CLOUD_KEY in env")
        uc.identity.service_account.use(key_id=key_id, key=key)
        print("service account set")
        return
    uc.identity.user_login.use()
    state = uc.identity.user_login.get_authentication_state()
    if state != uc.identity.user_login.Authentication_State.LOGGED_IN:
        print("opening the browser for Unity Cloud sign-in...", file=sys.stderr)
        uc.identity.user_login.login()
    print("auth state:", uc.identity.user_login.get_authentication_state())


def cmd_whoami(args):
    _init(args)
    info = uc.identity.user_login.get_current_user_info()
    print(info)


def cmd_projects(args):
    _init(args)
    orgs = uc.identity.get_organization_list()
    for org in orgs:
        print(f"org {org.id}  {getattr(org, 'name', '')}")
        try:
            projects = uc.identity.get_project_list(org_id=org.id)
            for p in projects:
                print(f"  project {p.id}  {getattr(p, 'name', '')}")
        except Exception as e:
            print(f"  (projects: {e})")


def cmd_assets(args):
    _init(args)
    assets = uc.assets.get_asset_list(org_id=args.org, project_id=args.project)
    for a in assets:
        print(f"{a.id}  {getattr(a, 'name', '')}  {getattr(a, 'type', '')}")


def cmd_search(args):
    _init(args)
    assets = uc.assets.search_assets_in_projects(
        org_id=args.org,
        project_ids=[args.project],
        include_filter=args.filter or {},
        limit_to=args.limit,
    )
    for a in assets:
        print(f"{a.id}  {getattr(a, 'name', '')}  {getattr(a, 'type', '')}")


def cmd_upload(args):
    _init(args)
    info = uc.models.FileUploadInformation(
        org_id=args.org,
        project_id=args.project,
        asset_id=args.asset,
        asset_version=args.version,
        dataset_id=args.dataset,
        file_name=os.path.basename(args.file),
        file_path=args.file,
    )
    ok = uc.assets.upload_file(info)
    print("uploaded" if ok else "upload failed")


def cmd_download(args):
    _init(args)
    info = uc.models.FileDownloadInformation(
        org_id=args.org,
        project_id=args.project,
        asset_id=args.asset,
        asset_version=args.version,
        dataset_id=args.dataset,
        file_name=args.file,
        download_path=args.out or ".",
    )
    ok = uc.assets.download_file(info)
    print("downloaded" if ok else "download failed")


def cmd_datasets(args):
    _init(args)
    ds = uc.assets.get_dataset_list(
        org_id=args.org, project_id=args.project,
        asset_id=args.asset, asset_version=args.version,
    )
    for d in ds:
        print(f"{d.id}  {getattr(d, 'name', '')}")


def main():
    parent = argparse.ArgumentParser(add_help=False)
    parent.add_argument("--service-account", action="store_true",
                        help="headless auth via UNITY_CLOUD_KEY_ID + UNITY_CLOUD_KEY")
    p = argparse.ArgumentParser(prog="unity-cloud", description=__doc__,
                                parents=[parent])
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("auth", parents=[parent], help="sign in (browser OAuth) or set a service account").set_defaults(fn=cmd_auth)
    sub.add_parser("whoami", parents=[parent], help="current auth state + user info").set_defaults(fn=cmd_whoami)

    pp = sub.add_parser("projects", parents=[parent], help="list organizations + projects")
    pp.set_defaults(fn=cmd_projects)

    pa = sub.add_parser("assets", parents=[parent], help="list assets in a project")
    pa.add_argument("--org", required=True)
    pa.add_argument("--project", required=True)
    pa.set_defaults(fn=cmd_assets)

    ps = sub.add_parser("search", parents=[parent], help="search assets")
    ps.add_argument("--org", required=True)
    ps.add_argument("--project", required=True)
    ps.add_argument("--filter", type=dict, default={})
    ps.add_argument("--limit", type=int, default=0)
    ps.set_defaults(fn=cmd_search)

    pd = sub.add_parser("datasets", parents=[parent], help="list datasets of an asset version")
    pd.add_argument("--org", required=True)
    pd.add_argument("--project", required=True)
    pd.add_argument("--asset", required=True)
    pd.add_argument("--version", required=True)
    pd.set_defaults(fn=cmd_datasets)

    pu = sub.add_parser("upload", parents=[parent], help="upload a file into an asset dataset")
    pu.add_argument("--org", required=True)
    pu.add_argument("--project", required=True)
    pu.add_argument("--asset", required=True)
    pu.add_argument("--version", required=True)
    pu.add_argument("--dataset", required=True)
    pu.add_argument("--file", required=True)
    pu.set_defaults(fn=cmd_upload)

    pg = sub.add_parser("download", parents=[parent], help="download a file from an asset dataset")
    pg.add_argument("--org", required=True)
    pg.add_argument("--project", required=True)
    pg.add_argument("--asset", required=True)
    pg.add_argument("--version", required=True)
    pg.add_argument("--dataset", required=True)
    pg.add_argument("--file", required=True)
    pg.add_argument("--out", default=".")
    pg.set_defaults(fn=cmd_download)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
