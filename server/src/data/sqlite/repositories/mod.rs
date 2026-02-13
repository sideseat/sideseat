//! SQLite repositories
//!
//! Types (UserRow, ProjectRow, etc.) should be imported from `crate::data::types`.

pub mod api_key;
pub mod auth_method;
pub mod favorite;
pub mod file;
pub mod membership;
pub mod organization;
pub mod project;
pub mod user;

pub use api_key::{
    create_api_key, delete_api_key, delete_for_org as delete_api_keys_for_org,
    get_by_hash as get_api_key_by_hash, get_hashes_for_org as get_api_key_hashes_for_org,
    list_for_org as list_api_keys_for_org, touch_api_key,
};
pub use auth_method::{
    create_auth_method, delete_auth_method, find_by_oauth, get_auth_method, get_bootstrap_method,
    list_for_user as list_auth_methods_for_user,
};
pub use favorite::{
    add_favorite, check_favorites, check_span_favorites, count_favorites,
    delete_favorites_by_entity, list_all_favorite_ids, remove_favorite,
};
pub use file::{
    decrement_ref_count, delete_file, delete_project_files, delete_trace_files, file_exists,
    get_file, get_file_hashes_for_traces, get_orphan_files, get_project_storage_bytes,
    insert_trace_file, upsert_file,
};
pub use membership::{
    add_member, get_member_with_user, get_membership, has_min_role_level, list_members,
    remove_member_atomic, update_role_atomic,
};
pub use organization::{
    create_organization, create_organization_with_owner, delete_organization, get_organization,
    list_for_user as list_orgs_for_user, list_project_ids, update_organization,
};
pub use project::{
    create_project, delete_project, get_project, list_for_org, list_for_user, list_projects,
    update_project,
};
pub use user::{
    create_user, delete_user, get_by_email, get_user, is_default_user, is_last_owner_of_any_org,
    update_user,
};
