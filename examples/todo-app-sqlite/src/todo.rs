use cfg_if::cfg_if;
use http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode};
use leptos::*;
use leptos_meta::*;
use leptos_router::*;
use serde::{Deserialize, Serialize};

cfg_if! {
    if #[cfg(feature = "ssr")] {
        use sqlx::{Connection, SqliteConnection};

        pub async fn db() -> Result<SqliteConnection, ServerFnError> {
            Ok(SqliteConnection::connect("sqlite:Todos.db").await.map_err(|e| ServerFnError::ServerError(e.to_string()))?)
        }

        pub fn register_server_functions() {
            _ = GetTodos::register();
            _ = AddTodo::register();
            _ = DeleteTodo::register();
        }

        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
        pub struct Todo {
            id: u16,
            title: String,
            completed: bool,
        }
    } else {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub struct Todo {
            id: u16,
            title: String,
            completed: bool,
        }
    }
}

#[server(GetTodos, "/api")]
pub async fn get_todos(cx: Scope) -> Result<Vec<Todo>, ServerFnError> {
    // this is just an example of how to access server context injected in the handlers
    let req =
        use_context::<actix_web::HttpRequest>(cx).expect("couldn't get HttpRequest from context");
    println!("\ncalling server fn");
    println!("  req.path = {:?}", req.path());

    use futures::TryStreamExt;

    let mut conn = db().await?;

    // fake API delay
    std::thread::sleep(std::time::Duration::from_millis(350));

    let mut todos = Vec::new();
    let mut rows = sqlx::query_as::<_, Todo>("SELECT * FROM todos").fetch(&mut conn);
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|e| ServerFnError::ServerError(e.to_string()))?
    {
        todos.push(row);
    }

    // Add a random header(because why not)
    let mut res_headers = HeaderMap::new();
    res_headers.insert(SET_COOKIE, HeaderValue::from_str("fizz=buzz").unwrap());

    let res_parts = leptos_actix::ResponseParts {
        headers: res_headers.into(),
        status: Some(StatusCode::IM_A_TEAPOT),
    };

    let res_options_outer = use_context::<leptos_actix::ResponseOptions>(cx);
    if let Some(res_options) = res_options_outer {
        println!("Setting Options");
        res_options.overwrite(res_parts).await;
    }

    Ok(todos)
}

#[server(AddTodo, "/api")]
pub async fn add_todo(title: String) -> Result<(), ServerFnError> {
    let mut conn = db().await?;

    // fake API delay
    std::thread::sleep(std::time::Duration::from_millis(350));

    sqlx::query("INSERT INTO todos (title, completed) VALUES ($1, false)")
        .bind(title)
        .execute(&mut conn)
        .await
        .map(|_| ())
        .map_err(|e| ServerFnError::ServerError(e.to_string()))
}

#[server(DeleteTodo, "/api")]
pub async fn delete_todo(id: u16) -> Result<(), ServerFnError> {
    let mut conn = db().await?;

    sqlx::query("DELETE FROM todos WHERE id = $1")
        .bind(id)
        .execute(&mut conn)
        .await
        .map(|_| ())
        .map_err(|e| ServerFnError::ServerError(e.to_string()))
}

#[component]
pub fn TodoApp(cx: Scope) -> Element {
    view! {
        cx,
        <div>
            <Stylesheet href="/style.css"/>
            <Router>
                <header>
                    <h1>"My Tasks"</h1>
                </header>
                <main>
                    <Routes>
                        <Route path="" element=|cx| view! {
                            cx,
                            <Todos/>
                        }/>
                    </Routes>
                </main>
            </Router>
        </div>
    }
}

#[component]
pub fn Todos(cx: Scope) -> Element {
    let add_todo = create_server_multi_action::<AddTodo>(cx);
    let delete_todo = create_server_action::<DeleteTodo>(cx);
    let submissions = add_todo.submissions();

    // track mutations that should lead us to refresh the list
    let add_changed = add_todo.version;
    let todo_deleted = delete_todo.version;

    // list of todos is loaded from the server in reaction to changes
    let todos = create_resource(
        cx,
        move || (add_changed(), todo_deleted()),
        move |_| get_todos(cx),
    );

    view! {
        cx,
        <div>
            <MultiActionForm action=add_todo>
                <label>
                    "Add a Todo"
                    <input type="text" name="title"/>
                </label>
                <input type="submit" value="Add"/>
            </MultiActionForm>
            <div>
                <Transition fallback=view! {cx, <p>"Loading..."</p> }>
                    {
                        let delete_todo = delete_todo.clone();
                        move || {
                        let existing_todos = {
                            let delete_todo = delete_todo.clone();
                            move || {
                                todos
                                .read()
                                .map({
                                    let delete_todo = delete_todo.clone();
                                    move |todos| match todos {
                                        Err(e) => {
                                            vec![view! { cx, <pre class="error">"Server Error: " {e.to_string()}</pre>}]
                                        }
                                        Ok(todos) => {
                                            if todos.is_empty() {
                                                vec![view! { cx, <p>"No tasks were found."</p> }]
                                            } else {
                                                todos
                                                    .into_iter()
                                                    .map({
                                                        let delete_todo = delete_todo.clone();
                                                        move |todo| {
                                                            let delete_todo = delete_todo.clone();
                                                            view! {
                                                                cx,
                                                                <li>
                                                                    {todo.title}
                                                                    <ActionForm action=delete_todo.clone()>
                                                                        <input type="hidden" name="id" value=todo.id/>
                                                                        <input type="submit" value="X"/>
                                                                    </ActionForm>
                                                                </li>
                                                            }
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                            }
                                        }
                                    }
                                })
                                .unwrap_or_default()
                            }
                        };

                        let pending_todos = move || {
                            submissions
                            .get()
                            .into_iter()
                            .filter(|submission| submission.pending().get())
                            .map(|submission| {
                                view! {
                                    cx,
                                    <li class="pending">{move || submission.input.get().map(|data| data.title) }</li>
                                }
                            })
                            .collect::<Vec<_>>()
                        };

                        view! {
                            cx,
                            <ul>
                                <div>{existing_todos}</div>
                                <div>{pending_todos}</div>
                            </ul>
                        }
                    }
                }
                </Transition>
            </div>
        </div>
    }
}
