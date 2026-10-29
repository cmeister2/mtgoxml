//! URL resolution independent of the host operating system's path rules.

use crate::{Error, Url};

/// Require a network location whose fragment cannot affect resource identity.
pub(crate) fn check_url(url: &Url) -> Result<(), Error> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err(Error::UnsupportedUrl(url.to_string()));
    }
    Ok(())
}

/// Resolve a URI codebase while honoring Windows directory separators.
pub(crate) fn codebase_url(base: &Url, reference: &str, mapped: bool) -> Result<Url, Error> {
    if reference.trim().is_empty() {
        return Err(Error::InvalidPath(reference.to_owned()));
    }
    let mut url = base.join(&reference.replace('\\', "/"))?;
    check_url(&url)?;
    if mapped {
        append_deploy(&mut url);
    }
    Ok(url)
}

/// Resolve literal file names, percent-encoding URL metacharacters in each segment.
pub(crate) fn file_url(base: &Url, path: &str, mapped: bool) -> Result<Url, Error> {
    if path
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || path.contains([':', '\0'])
    {
        return Err(Error::InvalidPath(path.to_owned()));
    }
    let mut url = base.join(".")?;
    {
        // Encode each filename segment literally, then release the path borrow
        // before applying ClickOnce's extension mapping.
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| Error::InvalidPath(path.to_owned()))?;
        segments.pop_if_empty();
        for segment in path.split('/') {
            segments.push(segment);
        }
    }
    if mapped {
        append_deploy(&mut url);
    }
    Ok(url)
}

/// Add ClickOnce's extension to the path, preserving an existing query string.
fn append_deploy(url: &mut Url) {
    url.set_path(&format!("{}.deploy", url.path()));
}
