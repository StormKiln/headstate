/// Where Headstate lives on GitHub. The one place the app spells it.
///
/// The repository moved owners, and four links still named the old one
/// (#1575). GitHub redirects a moved repository's pages, but a link that
/// relies on a redirect is one rename away from a 404 -- and an issue
/// link that loses its query string on the way opens an empty form.
/// Every link to the repository is built from this constant.
export const REPO_SLUG = "StormKiln/headstate";

export const REPO_URL = `https://github.com/${REPO_SLUG}`;

/// The latest release's page.
export const LATEST_RELEASE_URL = `${REPO_URL}/releases/latest`;

/// A new issue. The report adds `template=` and the form's fields.
export const NEW_ISSUE_URL = `${REPO_URL}/issues/new`;
