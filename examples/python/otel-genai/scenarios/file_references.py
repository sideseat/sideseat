from tools import respond

from harness import Run, content


def run(run: Run) -> None:
    # The document is uploaded first, and the request names it by the id the upload returned.
    uploaded = run.llm.client.files.create(
        file=("task.pdf", run.asset("task.pdf").read_bytes(), "application/pdf"),
        purpose="user_data",
    )
    prompt = [
        {"type": "input_text", "text": content.FILE_REFERENCES},
        {"type": "input_image", "image_url": content.IMAGE_URL},
        {"type": "input_file", "file_id": uploaded.id},
    ]
    with run.trace():
        print(respond(run.llm, [{"role": "user", "content": prompt}]))
