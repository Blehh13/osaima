import os
import chromadb
from langchain_community.document_loaders import DirectoryLoader, TextLoader, UnstructuredMarkdownLoader
from langchain_text_splitters import RecursiveCharacterTextSplitter
from sentence_transformers import SentenceTransformer

# Setup paths
BASE_DIR = os.environ.get("OSAIMA_ROOT", os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))
DB_DIR = os.path.join(BASE_DIR, "ai-core", "rag-engine", "chroma_db")

print("Initializing ChromaDB at:", DB_DIR)
client = chromadb.PersistentClient(path=DB_DIR)

# Get or create collection
collection = client.get_or_create_collection(
    name="osaima_knowledge",
    metadata={"hnsw:space": "cosine"}
)

print("Loading embedding model (all-MiniLM-L6-v2)...")
model = SentenceTransformer('all-MiniLM-L6-v2')

print(f"Loading markdown and text files from {BASE_DIR}...")
text_loader_kwargs={'autodetect_encoding': True}
loaders = [
    DirectoryLoader(BASE_DIR, glob="**/*.md", loader_cls=UnstructuredMarkdownLoader, show_progress=True),
    DirectoryLoader(BASE_DIR, glob="**/*.txt", loader_cls=TextLoader, loader_kwargs=text_loader_kwargs, show_progress=True),
]

documents = []
for loader in loaders:
    try:
        documents.extend(loader.load())
    except Exception as e:
        print(f"Error loading some files: {e}")

print(f"Loaded {len(documents)} documents.")

# Split documents
text_splitter = RecursiveCharacterTextSplitter(
    chunk_size=512,
    chunk_overlap=50,
    separators=["\n\n", "\n", " ", ""]
)

chunks = text_splitter.split_documents(documents)
print(f"Split into {len(chunks)} chunks.")

if not chunks:
    print("No chunks to index. Exiting.")
    exit(0)

# Extract texts and metadatas
texts = [chunk.page_content for chunk in chunks]
metadatas = [chunk.metadata for chunk in chunks]
ids = [f"chunk_{i}" for i in range(len(chunks))]

# Embed and store
print("Embedding chunks and storing in ChromaDB...")
embeddings = model.encode(texts).tolist()

collection.upsert(
    documents=texts,
    embeddings=embeddings,
    metadatas=metadatas,
    ids=ids
)

print("Indexing complete!")
